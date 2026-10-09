//! The phone's name for itself in the user's list of devices: its maker
//! and model on Android ("Google Pixel 8"; the host name there is
//! "localhost"), the kind of device on iOS. Elsewhere `AppClient` names
//! the machine by its host name.

#[cfg(target_os = "android")]
pub fn phone_model() -> Option<String> {
    fn property(name: &std::ffi::CStr) -> Option<String> {
        let mut value = [0u8; 92]; // PROP_VALUE_MAX
                                   // SAFETY: the buffer is PROP_VALUE_MAX bytes, as the call requires.
        let n = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr().cast()) };
        let text = String::from_utf8_lossy(&value[..n.max(0) as usize]);
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
    let model = property(c"ro.product.model")?;
    Some(match property(c"ro.product.manufacturer") {
        Some(maker) if !model.to_lowercase().starts_with(&maker.to_lowercase()) => {
            let mut maker = maker;
            if let Some(first) = maker.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            format!("{maker} {model}")
        }
        _ => model,
    })
}

#[cfg(target_os = "ios")]
pub fn phone_model() -> Option<String> {
    let name = c"hw.machine";
    let mut size = 0usize;
    // SAFETY: sysctlbyname first reports the size, then fills a buffer of it.
    let machine = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        );
        let mut buf = vec![0u8; size];
        if libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        String::from_utf8_lossy(&buf).into_owned()
    };
    Some(
        if machine.starts_with("iPad") {
            "iPad"
        } else if machine.starts_with("iPhone") {
            "iPhone"
        } else {
            "iOS"
        }
        .to_string(),
    )
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn phone_model() -> Option<String> {
    None
}
