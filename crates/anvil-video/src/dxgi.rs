//! Screens on Windows, through DXGI's Desktop Duplication (Windows 8 and
//! later): each display a graphics adapter drives, captured as BGRA on the
//! source's own thread — a frame the display has changed copied from the
//! GPU, the last one repeated while it has not.
//!
//! Nothing is asked of the user: a desktop application may duplicate the
//! desktop it runs on. The secure desktop (a UAC prompt, the lock screen)
//! cannot be: the share shows its last picture until it comes back. The
//! pointer is not drawn into the picture.

use std::time::{Duration, Instant};

use anvil_core::video::host::{Resolution, ScreenInfo};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::camera::Capture;
use crate::picture::Picture;
use crate::screen::Screens;

/// The PC's displays.
#[derive(Debug, Clone, Copy, Default)]
pub struct DxgiScreens;

/// One display: the adapter that drives it, and the output.
struct Display {
    /// The output's device name (`\\.\DISPLAY1`): its id.
    id: String,
    width: u32,
    height: u32,
    primary: bool,
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
}

/// The displays on the desktop, the primary one first.
fn displays() -> Vec<Display> {
    let mut out = Vec::new();
    // SAFETY: DXGI's factory and its enumerations, each result checked.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                if let Ok(desc) = output.GetDesc() {
                    if desc.AttachedToDesktop.as_bool() {
                        let r = desc.DesktopCoordinates;
                        let name = String::from_utf16_lossy(&desc.DeviceName);
                        out.push(Display {
                            id: name.trim_end_matches('\0').to_string(),
                            width: (r.right - r.left).max(0) as u32,
                            height: (r.bottom - r.top).max(0) as u32,
                            // The primary display holds the desktop's origin.
                            primary: r.left == 0 && r.top == 0,
                            adapter: adapter.clone(),
                            output,
                        });
                    }
                }
                o += 1;
            }
            a += 1;
        }
    }
    out.sort_by_key(|d| !d.primary);
    out
}

impl Screens for DxgiScreens {
    fn list(&self) -> Vec<ScreenInfo> {
        displays()
            .iter()
            .enumerate()
            .map(|(i, d)| ScreenInfo {
                id: d.id.clone(),
                name: format!("Display {} ({}×{})", i + 1, d.width, d.height),
            })
            .collect()
    }

    fn open(
        &self,
        id: Option<&str>,
        _want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        let list = displays();
        let display = match id {
            Some(id) => list.iter().find(|d| d.id == id),
            None => list.first(),
        }
        .ok_or_else(|| format!("no display {}", id.unwrap_or("here")))?;
        // The duplication is made on the capture's thread, at its first
        // picture: Direct3D's objects stay with the thread that uses them.
        Ok(Box::new(DxgiCapture {
            id: display.id.clone(),
            every: Duration::from_secs(1) / fps.clamp(1, 30),
            next_at: Instant::now(),
            duplication: None,
            last: None,
            failed: false,
        }))
    }
}

/// A display being duplicated.
struct Duplication {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    /// A CPU-readable copy of the desktop, made once its size is known.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

// SAFETY: the Direct3D objects are made and used on the capture's own
// thread only; the capture moves to that thread before it starts them.
unsafe impl Send for Duplication {}

struct DxgiCapture {
    id: String,
    every: Duration,
    next_at: Instant,
    duplication: Option<Duplication>,
    /// The last picture: a display that has not changed sends none.
    last: Option<Picture>,
    failed: bool,
}

/// Start duplicating the display `id`.
fn duplicate(id: &str) -> Result<Duplication, String> {
    let display = displays()
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("display {id} has gone"))?;
    // SAFETY: COM for this thread, a device on the display's adapter, and
    // the output's duplication on it — each result checked.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            &display.adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| format!("a Direct3D device: {e}"))?;
        let (device, context) = device.zip(context).ok_or("no Direct3D device")?;
        let output: IDXGIOutput1 = display
            .output
            .cast()
            .map_err(|e| format!("the display cannot be duplicated: {e}"))?;
        let duplication = output
            .DuplicateOutput(&device)
            .map_err(|e| format!("the display cannot be duplicated here: {e}"))?;
        Ok(Duplication {
            device,
            context,
            duplication,
            staging: None,
        })
    }
}

/// What a frame wait gave.
enum Got {
    Picture(Picture),
    /// Nothing new (the display is still, or only the pointer moved).
    Same,
    /// The duplication must be made again (the mode changed, the secure
    /// desktop came and went).
    Lost,
}

impl Duplication {
    /// The next frame, waiting up to `wait` for one.
    fn frame(&mut self, wait: Duration) -> Result<Got, String> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        // SAFETY: a frame acquired is released below, whatever happens to
        // it; the staging texture is mapped and unmapped around the read.
        unsafe {
            match self.duplication.AcquireNextFrame(
                wait.as_millis() as u32,
                &mut info,
                &mut resource,
            ) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Got::Same),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => return Ok(Got::Lost),
                Err(e) => return Err(format!("the display's frame: {e}")),
            }
            let got = self.copy(&info, resource);
            let _ = self.duplication.ReleaseFrame();
            got
        }
    }

    unsafe fn copy(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<windows::Win32::Graphics::Dxgi::IDXGIResource>,
    ) -> Result<Got, String> {
        // Only the pointer changed: no new picture.
        if info.LastPresentTime == 0 {
            return Ok(Got::Same);
        }
        let texture: ID3D11Texture2D = resource
            .ok_or("no frame")?
            .cast()
            .map_err(|e| format!("the frame's texture: {e}"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(format!(
                "the display's format {:?} is not BGRA",
                desc.Format
            ));
        }
        let (w, h) = (desc.Width, desc.Height);
        if !matches!(&self.staging, Some((_, sw, sh)) if (*sw, *sh) == (w, h)) {
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: desc.SampleDesc,
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|e| format!("a readable copy of the display: {e}"))?;
            self.staging = Some((staging.ok_or("no readable copy")?, w, h));
        }
        let Some((staging, _, _)) = &self.staging else {
            return Err("no readable copy".into());
        };
        self.context.CopyResource(staging, &texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("reading the display: {e}"))?;
        let pitch = mapped.RowPitch as usize;
        let picture = (!mapped.pData.is_null() && w >= 2 && h >= 2 && pitch >= w as usize * 4)
            .then(|| {
                let bytes =
                    std::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * h as usize);
                Picture::from_bgrx(bytes, pitch, w & !1, h & !1)
            });
        self.context.Unmap(staging, 0);
        Ok(picture.map(Got::Picture).unwrap_or(Got::Same))
    }
}

impl Capture for DxgiCapture {
    fn next(&mut self) -> Option<Picture> {
        if self.failed {
            return None;
        }
        let now = Instant::now();
        if self.next_at > now {
            std::thread::sleep(self.next_at - now);
        }
        self.next_at = (self.next_at + self.every).max(Instant::now());
        for _ in 0..3 {
            if self.duplication.is_none() {
                match duplicate(&self.id) {
                    Ok(d) => self.duplication = Some(d),
                    Err(e) if self.last.is_some() => {
                        // The secure desktop: the last picture until the
                        // desktop comes back.
                        tracing::debug!("screen: {e}");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("screen: {e}");
                        self.failed = true;
                        return None;
                    }
                }
            }
            // The first picture is waited for; later, a still display
            // repeats the last one.
            let wait = if self.last.is_none() {
                Duration::from_millis(500)
            } else {
                Duration::ZERO
            };
            let got = self.duplication.as_mut()?.frame(wait);
            match got {
                Ok(Got::Picture(p)) => {
                    self.last = Some(p);
                    break;
                }
                Ok(Got::Same) => {
                    if self.last.is_some() {
                        break;
                    }
                }
                Ok(Got::Lost) => self.duplication = None,
                Err(e) => {
                    tracing::warn!("screen: {e}");
                    self.failed = true;
                    return None;
                }
            }
        }
        let mut p = self.last.clone()?;
        p.captured = Instant::now();
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The displays listed open by their ids, and an unknown one does not.
    /// Where the desktop can be duplicated (an interactive session; CI's
    /// service session may not), pictures come at the display's size.
    #[test]
    fn the_displays_listed_open_and_are_captured_where_they_can_be() {
        let screens = DxgiScreens.list();
        for s in &screens {
            assert!(s.id.starts_with(r"\\.\"), "{s:?}");
            assert!(DxgiScreens.open(Some(&s.id), Resolution::P1080, 10).is_ok());
        }
        assert!(DxgiScreens
            .open(Some("no-such-display"), Resolution::P1080, 10)
            .is_err());
        let Some(first) = screens.first() else {
            eprintln!("skipped: no display here");
            return;
        };
        if let Err(e) = duplicate(&first.id) {
            eprintln!("skipped: {e}");
            return;
        }
        let mut capture = DxgiScreens.open(None, Resolution::P1080, 10).unwrap();
        for _ in 0..3 {
            let p = capture.next().expect("a picture");
            assert!(p.width >= 2 && p.height >= 2, "{p:?}");
        }
    }
}
