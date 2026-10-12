//! Windows: the default device for communications. Windows keeps two
//! defaults per direction — one for everything, one for calls — and a
//! headset user sets the second (Sound settings, "Default Communication
//! Device"); cpal knows only the first. A call follows the one for calls.

use windows::core::BSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED, STGM_READ,
};

/// The name (as cpal names it: the friendly name) of the default capture
/// (`input`) or playback device for communications, if Windows says.
pub fn default_name(input: bool) -> Option<String> {
    // SAFETY: COM calls on this thread, initialised as cpal initialises its
    // own; every interface is released when dropped.
    unsafe {
        // Already initialised on this thread is fine.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let flow = if input { eCapture } else { eRender };
        let device = devices
            .GetDefaultAudioEndpoint(flow, eCommunications)
            .ok()?;
        let properties = device.OpenPropertyStore(STGM_READ).ok()?;
        let name = properties.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let name = BSTR::try_from(&name).ok()?.to_string();
        (!name.is_empty()).then_some(name)
    }
}
