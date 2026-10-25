//! The brand on the system while the app runs (`docs/MANAGED_INSTALLS.md`
//! §2.4): the icon files the window, the tray and the Dock show, made from
//! the brand's image; on Windows the shortcuts Anvil's installer made and
//! the notifications' app id, renamed and re-iconed; on Linux a user-level
//! `.desktop` file over the packaged one. Without a brand, Anvil's again.

use std::io;
use std::path::{Path, PathBuf};

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, RgbaImage};
use sha2::{Digest, Sha256};

/// The tray's states, by the names of the app's own tray icons, each with
/// the colour of its dot (`app/tool/icons.py`).
pub const TRAY_STATES: [(&str, [u8; 3]); 5] = [
    ("offline", [0x9A, 0xA5, 0xB1]),
    ("ready", [0x1F, 0x9D, 0x55]),
    ("dnd", [0xD6, 0x45, 0x45]),
    ("ringing", [0xD9, 0x9A, 0x1E]),
    ("in_call", [0x0E, 0x8A, 0x7E]),
];

/// The sizes in an ICO: the window's, the taskbar's and the Start menu's.
const ICO_SIZES: [u32; 5] = [16, 24, 32, 48, 256];
/// The tray's, where Windows picks by the screen's scale.
const TRAY_ICO_SIZES: [u32; 4] = [16, 20, 24, 32];
/// A PNG's: the Dock's and a Linux menu's icon, and the tray's elsewhere
/// (the size of the app's own).
const PNG_SIZE: u32 = 256;
const TRAY_PNG_SIZE: u32 = 64;

/// The icon files a brand's image was made into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    /// The window's: an ICO on Windows, a PNG elsewhere.
    pub window: PathBuf,
    /// A PNG, for the Dock and a Linux menu.
    pub png: PathBuf,
    /// The tray's, one per state in [`TRAY_STATES`]' order: the brand's
    /// icon with a dot in the state's colour.
    pub tray: Vec<(&'static str, PathBuf)>,
}

/// The icon files for `image` (a PNG, JPEG or WebP) in `dir`, made once
/// and named by its hash: Windows keeps an icon by its path, so a new image
/// at an old path would go unseen. Those of an earlier image go.
pub fn look(dir: &Path, image: &[u8]) -> io::Result<Look> {
    look_for(dir, image, cfg!(windows))
}

fn look_for(dir: &Path, image: &[u8], windows: bool) -> io::Result<Look> {
    let key: String = Sha256::digest(image)[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let ext = if windows { "ico" } else { "png" };
    let look = Look {
        window: dir.join(format!("icon-{key}.{ext}")),
        png: dir.join(format!("icon-{key}.png")),
        tray: TRAY_STATES
            .iter()
            .map(|(state, _)| (*state, dir.join(format!("tray-{key}-{state}.{ext}"))))
            .collect(),
    };
    let made = [&look.window, &look.png]
        .into_iter()
        .chain(look.tray.iter().map(|(_, p)| p))
        .all(|p| p.is_file());
    if made {
        return Ok(look);
    }
    let source = image::load_from_memory(image).map_err(invalid)?;
    std::fs::create_dir_all(dir)?;
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with("icon-") || name.starts_with("tray-")) && !name.contains(&key) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    write(&look.png, &encode_png(&square(&source, PNG_SIZE))?)?;
    if windows {
        let sizes = ICO_SIZES.map(|s| (s, square(&source, s)));
        write(&look.window, &encode_ico(&sizes)?)?;
    }
    for ((_, colour), (_, path)) in TRAY_STATES.iter().zip(&look.tray) {
        let bytes = if windows {
            let sizes = TRAY_ICO_SIZES.map(|s| (s, with_dot(square(&source, s), *colour)));
            encode_ico(&sizes)?
        } else {
            encode_png(&with_dot(square(&source, TRAY_PNG_SIZE), *colour))?
        };
        write(path, &bytes)?;
    }
    Ok(look)
}

fn invalid(e: image::ImageError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Written beside, then moved in place: a half-written icon is never shown.
fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// The image fitted in a `size` square, centred, on transparency.
fn square(image: &DynamicImage, size: u32) -> RgbaImage {
    let fitted = image.resize(size, size, FilterType::Lanczos3).to_rgba8();
    let mut canvas = RgbaImage::new(size, size);
    let x = (size - fitted.width()) / 2;
    let y = (size - fitted.height()) / 2;
    imageops::overlay(&mut canvas, &fitted, x.into(), y.into());
    canvas
}

/// A dot in the bottom right corner, cut out of the icon by a clear ring
/// so it reads on any taskbar.
fn with_dot(mut icon: RgbaImage, colour: [u8; 3]) -> RgbaImage {
    let size = icon.width() as f32;
    let radius = size * 0.22;
    let gap = (size * 0.07).max(1.0);
    let centre = size - radius - size * 0.02;
    for (x, y, pixel) in icon.enumerate_pixels_mut() {
        let d = ((x as f32 + 0.5 - centre).powi(2) + (y as f32 + 0.5 - centre).powi(2)).sqrt();
        // How much of the pixel the ring and the dot cover, anti-aliased.
        let cut = (radius + gap - d + 0.5).clamp(0.0, 1.0);
        let dot = (radius - d + 0.5).clamp(0.0, 1.0);
        let under = pixel[3] as f32 / 255.0 * (1.0 - cut);
        let alpha = dot + under * (1.0 - dot);
        if alpha <= 0.0 {
            *pixel = image::Rgba([0, 0, 0, 0]);
            continue;
        }
        for c in 0..3 {
            let mixed = (colour[c] as f32 * dot + pixel[c] as f32 * under * (1.0 - dot)) / alpha;
            pixel[c] = mixed.round() as u8;
        }
        pixel[3] = (alpha * 255.0).round() as u8;
    }
    icon
}

fn encode_png(image: &RgbaImage) -> io::Result<Vec<u8>> {
    let mut out = io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, ImageFormat::Png)
        .map_err(invalid)?;
    Ok(out.into_inner())
}

/// PNG images in an ICO, one per size.
fn encode_ico(images: &[(u32, RgbaImage)]) -> io::Result<Vec<u8>> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    let pngs = images
        .iter()
        .map(|(_, image)| encode_png(image))
        .collect::<io::Result<Vec<_>>>()?;
    let frames = images
        .iter()
        .zip(&pngs)
        .map(|((size, _), png)| {
            IcoFrame::with_encoded(png, *size, *size, image::ExtendedColorType::Rgba8)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    let mut out = Vec::new();
    IcoEncoder::new(&mut out)
        .encode_images(&frames)
        .map_err(invalid)?;
    Ok(out)
}

/// A name Windows takes for a file, as the installer makes the shortcuts'
/// (`app/packaging/profile.ps1`); Anvil's if nothing is left of it.
pub fn file_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| {
            !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control()
        })
        .collect();
    let kept = kept.trim().trim_end_matches('.').trim_end();
    if kept.is_empty() {
        "Anvil".into()
    } else {
        kept.into()
    }
}

/// Show `name` and `look` (a brand's), or Anvil's own with neither, where
/// the system keeps the app's name and icon outside the app: Windows'
/// shortcuts and app id, a Linux menu. What fails is logged and left.
pub fn apply(name: Option<&str>, look: Option<&Look>) {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    #[cfg(windows)]
    win::apply(name, look);
    #[cfg(target_os = "linux")]
    linux::apply(name, look);
    #[cfg(target_os = "macos")]
    let _ = (name, look);
}

/// `shortcuts.json` in the profile's directory: the shortcuts the
/// installer made, which the app may rename (and no others).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Shortcuts {
    #[serde(default)]
    shortcuts: Vec<PathBuf>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Shortcuts {
    fn read(path: &Path) -> Option<Shortcuts> {
        let bytes = std::fs::read(path).ok()?;
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        serde_json::from_slice(bytes)
            .map_err(|e| tracing::warn!(%e, path = %path.display(), "shortcuts.json is not read"))
            .ok()
    }

    /// With a byte order mark, as the installer writes it and reads it
    /// (Inno Setup takes a file without one for the system's code page).
    fn write(&self, path: &Path) -> io::Result<()> {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(serde_json::to_vec_pretty(self).map_err(io::Error::other)?);
        bytes.extend(b"\r\n");
        write(path, &bytes)
    }

    /// Each shortcut's path under `name`: the same folder, the new file.
    fn renamed(&self, name: &str) -> Vec<(PathBuf, PathBuf)> {
        let file = format!("{}.lnk", file_name(name));
        self.shortcuts
            .iter()
            .map(|old| (old.clone(), old.with_file_name(&file)))
            .collect()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use super::*;
    use ::windows::core::{Interface, HSTRING};
    use ::windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READWRITE,
    };
    use ::windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };

    /// The notifications' app id (`native_shell.dart`, `windows.iss`).
    const AUMID_KEY: &str = r"Software\Classes\AppUserModelId\Anvil.Softphone";

    pub(super) fn apply(name: Option<&str>, look: Option<&Look>) {
        let shown = name.unwrap_or("Anvil");
        let icon = look.map(|l| l.window.as_path());
        // Where the installer keeps its list, beside the app whatever
        // profile the app was given.
        if let Some(app) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            shortcuts(&app.join("profile").join("shortcuts.json"), shown, icon);
        }
        app_id(shown, icon);
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The name and icon a call's notification shows.
    fn app_id(name: &str, icon: Option<&Path>) {
        let key = wide(AUMID_KEY);
        let set = |value: &str, data: &str| {
            let value = wide(value);
            let data = wide(data);
            // SAFETY: the strings are NUL-terminated; the data's size in
            // bytes includes its NUL.
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            }
        };
        let mut status = set("DisplayName", name);
        status |= match icon {
            Some(icon) => set("IconUri", &icon.to_string_lossy()),
            None => {
                let value = wide("IconUri");
                // SAFETY: the strings are NUL-terminated. A value already
                // gone is no failure.
                unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
                ERROR_SUCCESS
            }
        };
        if status != ERROR_SUCCESS {
            tracing::warn!(status, "the notifications' name and icon not set");
        }
    }

    /// The installer's shortcuts named `name`, with `icon` or the app's
    /// own; `shortcuts.json` then lists them under their new names.
    fn shortcuts(list: &Path, name: &str, icon: Option<&Path>) {
        let Some(made) = Shortcuts::read(list) else {
            return;
        };
        // SAFETY: COM for this thread, undone below when this started it.
        let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut now = Vec::new();
        for (old, new) in made.renamed(name) {
            if !old.is_file() {
                continue;
            }
            match relink(&old, &new, icon) {
                Ok(()) => now.push(new),
                Err(e) => {
                    tracing::warn!(%e, shortcut = %old.display(), "a shortcut not renamed");
                    now.push(old);
                }
            }
        }
        if init.is_ok() {
            // SAFETY: paired with the CoInitializeEx that succeeded.
            unsafe { CoUninitialize() };
        }
        let now = Shortcuts { shortcuts: now };
        if now.shortcuts != made.shortcuts {
            if let Err(e) = now.write(list) {
                tracing::warn!(%e, "shortcuts.json not written");
            }
        }
    }

    /// The shortcut at `old` saved at `new` pointing at `icon` (empty: the
    /// program's own); left as it is when it already is.
    fn relink(old: &Path, new: &Path, icon: Option<&Path>) -> ::windows::core::Result<()> {
        let want = icon
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        // SAFETY: COM objects used on the thread that made them; buffers
        // are passed as slices.
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(&HSTRING::from(old.as_os_str()), STGM_READWRITE)?;
            let mut buf = [0u16; 1024];
            let mut index = 0;
            link.GetIconLocation(&mut buf, &mut index)?;
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let has = String::from_utf16_lossy(&buf[..len]);
            if old == new && has.eq_ignore_ascii_case(&want) {
                return Ok(());
            }
            link.SetIconLocation(&HSTRING::from(want.as_str()), 0)?;
            file.Save(&HSTRING::from(new.as_os_str()), true)?;
        }
        if old != new {
            let _ = std::fs::remove_file(old);
        }
        tracing::info!(shortcut = %new.display(), "a shortcut named for the brand");
        Ok(())
    }
}

#[cfg(any(target_os = "linux", test))]
mod linux {
    use super::*;

    /// The packaged entry's id (`app/packaging/linux.sh`).
    pub(super) const DESKTOP_ID: &str = "com.thevoiceguy.anvil.desktop";
    /// Marks the entry as the app's own, to change and take away.
    pub(super) const MARK: &str = "X-Anvil-Brand=true";

    #[cfg(target_os = "linux")]
    pub(super) fn apply(name: Option<&str>, look: Option<&Look>) {
        let Some(home) = dirs::data_dir() else {
            return;
        };
        let dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let packaged = dirs
            .split(':')
            .map(|d| Path::new(d).join("applications").join(DESKTOP_ID))
            .find(|p| p.is_file());
        let ours = home.join("applications").join(DESKTOP_ID);
        if let Err(e) = override_entry(
            &ours,
            packaged.as_deref(),
            name,
            look.map(|l| l.png.as_path()),
        ) {
            tracing::warn!(%e, path = %ours.display(), "the menu entry not named for the brand");
        }
    }

    /// The user's entry over `packaged`, with the brand's name and icon;
    /// gone without a brand. A user's entry the app did not write is kept.
    pub(super) fn override_entry(
        ours: &Path,
        packaged: Option<&Path>,
        name: Option<&str>,
        icon: Option<&Path>,
    ) -> io::Result<()> {
        let existing = std::fs::read_to_string(ours).ok();
        if existing
            .as_ref()
            .is_some_and(|e| !e.lines().any(|l| l.trim() == MARK))
        {
            return Ok(());
        }
        let wanted = match (packaged, name.is_some() || icon.is_some()) {
            (Some(packaged), true) => Some(entry(&std::fs::read_to_string(packaged)?, name, icon)),
            _ => None,
        };
        match (wanted, existing) {
            (Some(w), Some(e)) if w == e => Ok(()),
            (Some(w), _) => {
                std::fs::create_dir_all(ours.parent().unwrap_or(Path::new(".")))?;
                write(ours, w.as_bytes())
            }
            (None, Some(_)) => std::fs::remove_file(ours),
            (None, None) => Ok(()),
        }
    }

    /// The packaged entry with its name (every language's) and icon
    /// replaced, and marked.
    pub(super) fn entry(packaged: &str, name: Option<&str>, icon: Option<&Path>) -> String {
        let mut out = String::new();
        let mut main = false;
        for line in packaged.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                if main {
                    out.push_str(MARK);
                    out.push('\n');
                }
                main = t == "[Desktop Entry]";
            } else if main {
                let key = t.split('=').next().unwrap_or("").trim();
                let base = key.split('[').next().unwrap_or("");
                if name.is_some() && base == "Name" {
                    if key == "Name" {
                        out.push_str(&format!("Name={}\n", name.unwrap_or_default()));
                    }
                    continue;
                }
                if let (Some(icon), "Icon") = (icon, key) {
                    out.push_str(&format!("Icon={}\n", icon.display()));
                    continue;
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        if main {
            out.push_str(MARK);
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anvil-system-brand-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A `w` by `h` PNG of one colour.
    fn png(w: u32, h: u32, colour: [u8; 4]) -> Vec<u8> {
        encode_png(&RgbaImage::from_pixel(w, h, image::Rgba(colour))).unwrap()
    }

    #[test]
    fn a_brand_s_image_is_made_into_the_system_s_icons() {
        let dir = temp();
        let image = png(600, 300, [200, 10, 10, 255]);
        let look = look_for(&dir, &image, true).unwrap();
        let ico = image::load_from_memory(&std::fs::read(&look.window).unwrap()).unwrap();
        // The largest image in the ICO, the wide image fitted and centred.
        assert_eq!((ico.width(), ico.height()), (256, 256));
        let ico = ico.to_rgba8();
        assert_eq!(ico.get_pixel(0, 0)[3], 0);
        let middle = ico.get_pixel(128, 128).0;
        assert!(middle[0].abs_diff(200) <= 2 && middle[1] <= 12 && middle[3] == 255);
        let bytes = std::fs::read(&look.window).unwrap();
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            ICO_SIZES.len() as u16
        );
        assert_eq!(look.tray.len(), TRAY_STATES.len());
        for ((state, path), (name, colour)) in look.tray.iter().zip(TRAY_STATES) {
            assert_eq!(*state, name);
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(
                u16::from_le_bytes([bytes[4], bytes[5]]),
                TRAY_ICO_SIZES.len() as u16
            );
            let tray = image::load_from_memory(&bytes).unwrap().to_rgba8();
            let corner = tray.get_pixel(tray.width() * 3 / 4 + 1, tray.height() * 3 / 4 + 1);
            assert_eq!(&corner.0[..3], &colour, "the {state} dot");
        }
        let png = image::load_from_memory(&std::fs::read(&look.png).unwrap()).unwrap();
        assert_eq!(png.width(), PNG_SIZE);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_image_takes_new_files_and_the_old_go() {
        let dir = temp();
        let first = look_for(&dir, &png(64, 64, [1, 2, 3, 255]), false).unwrap();
        assert!(first.window.extension().unwrap() == "png" && first.window.is_file());
        let again = look_for(&dir, &png(64, 64, [1, 2, 3, 255]), false).unwrap();
        assert_eq!(first, again);
        let second = look_for(&dir, &png(64, 64, [4, 5, 6, 255]), false).unwrap();
        assert_ne!(first.window, second.window);
        assert!(!first.window.exists() && !first.tray[0].1.exists());
        assert!(second.window.is_file() && second.tray[4].1.is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_is_not_an_image_makes_no_icons() {
        let dir = temp();
        let e = look_for(&dir, b"<svg/>", true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_shortcut_s_name_is_one_windows_takes() {
        assert_eq!(file_name("Acme Voice"), "Acme Voice");
        assert_eq!(file_name("A/B: \"C\"?"), "AB C");
        assert_eq!(file_name("Acme Inc."), "Acme Inc");
        assert_eq!(file_name(" ?* "), "Anvil");
    }

    #[test]
    fn shortcuts_json_is_read_as_the_installer_writes_it_and_renamed() {
        let dir = temp();
        let list = dir.join("shortcuts.json");
        // As Inno Setup's SaveStringsToUTF8File writes it.
        std::fs::write(
            &list,
            "\u{feff}{\r\n  \"shortcuts\": [\r\n    \"C:\\\\Users\\\\Zoë\\\\Start Menu\\\\Anvil.lnk\",\r\n    \"C:\\\\Users\\\\Zoë\\\\Desktop\\\\Anvil.lnk\"\r\n  ]\r\n}\r\n",
        )
        .unwrap();
        let made = Shortcuts::read(&list).unwrap();
        assert_eq!(made.shortcuts.len(), 2);
        let renamed = made.renamed("Acme: Voice");
        assert_eq!(renamed[0].1.file_name().unwrap(), "Acme Voice.lnk");
        made.write(&list).unwrap();
        let text = std::fs::read(&list).unwrap();
        assert!(text.starts_with(b"\xEF\xBB\xBF"));
        // Each path alone on its line, quoted, as the installer reads them.
        let text = String::from_utf8_lossy(&text[3..]).into_owned();
        assert!(text
            .lines()
            .any(|l| l.trim().trim_end_matches(',') == r#""C:\\Users\\Zoë\\Desktop\\Anvil.lnk""#));
        std::fs::remove_dir_all(dir).unwrap();
    }

    const PACKAGED: &str = "[Desktop Entry]\nType=Application\nName=Anvil\nName[de]=Anvil\nExec=anvil\nIcon=com.thevoiceguy.anvil\nStartupWMClass=com.thevoiceguy.anvil\n\n[Desktop Action Quit]\nName=Quit\n";

    #[test]
    fn the_menu_entry_takes_the_brand_s_name_and_icon() {
        let icon = Path::new("/home/z/.local/share/Anvil/brand/icon-1.png");
        let entry = linux::entry(PACKAGED, Some("Acme Voice"), Some(icon));
        assert!(entry.contains("Name=Acme Voice\n"));
        assert!(!entry.contains("Name[de]"));
        assert!(entry.contains(&format!("Icon={}\n", icon.display())));
        assert!(
            entry.contains("Exec=anvil\n")
                && entry.contains("StartupWMClass=com.thevoiceguy.anvil")
        );
        // Marked in the main group; another group's name is its own.
        let (main, action) = entry.split_once("[Desktop Action Quit]").unwrap();
        assert!(main.contains(linux::MARK));
        assert!(action.contains("Name=Quit"));
    }

    #[test]
    fn the_menu_entry_goes_with_the_brand_and_a_user_s_own_is_kept() {
        let dir = temp();
        let packaged = dir.join("packaged.desktop");
        std::fs::write(&packaged, PACKAGED).unwrap();
        let ours = dir.join("applications").join(linux::DESKTOP_ID);
        linux::override_entry(&ours, Some(&packaged), Some("Acme"), None).unwrap();
        assert!(std::fs::read_to_string(&ours)
            .unwrap()
            .contains("Name=Acme"));
        linux::override_entry(&ours, Some(&packaged), None, None).unwrap();
        assert!(!ours.exists());
        // Nothing packaged (an AppImage): no entry of the app's own.
        linux::override_entry(&ours, None, Some("Acme"), None).unwrap();
        assert!(!ours.exists());
        std::fs::write(&ours, "[Desktop Entry]\nName=Mine\n").unwrap();
        linux::override_entry(&ours, Some(&packaged), Some("Acme"), None).unwrap();
        linux::override_entry(&ours, Some(&packaged), None, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&ours).unwrap(),
            "[Desktop Entry]\nName=Mine\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
