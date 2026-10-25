//! Windows' half of a managed install: the policy IT writes under
//! `Software\Policies\Anvil` (Group Policy, Intune) and the signed-in
//! user's principal name.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Security::Authentication::Identity::{GetUserNameExW, NameUserPrincipal};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

const KEY: &str = r"Software\Policies\Anvil";
const NAMES: [&str; 6] = [
    "Server",
    "LockServer",
    "LoginDomain",
    "SignIn",
    "UsernameFromSystem",
    "Updates",
];

pub(crate) enum RegValue {
    Text(String),
    Dword(u32),
}

/// The machine's policy, then the user's, each with the values it holds.
pub(crate) fn policies() -> Vec<(String, Vec<(String, RegValue)>)> {
    [(HKEY_LOCAL_MACHINE, "HKLM"), (HKEY_CURRENT_USER, "HKCU")]
        .into_iter()
        .map(|(root, name)| {
            let values = NAMES
                .iter()
                .filter_map(|n| read(root, n).map(|v| (n.to_string(), v)))
                .collect();
            (format!(r"{name}\{KEY}"), values)
        })
        .collect()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read(root: HKEY, name: &str) -> Option<RegValue> {
    let key = wide(KEY);
    let value = wide(name);
    let mut dword: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: the key and value names are NUL-terminated; the buffer is a
    // u32 whose size is passed.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut dword as *mut u32).cast(),
            &mut size,
        )
    };
    if status == ERROR_SUCCESS {
        return Some(RegValue::Dword(dword));
    }
    let mut size: u32 = 0;
    // SAFETY: a null buffer asks for the size only.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: the buffer holds `size` bytes, as passed.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(RegValue::Text(String::from_utf16_lossy(&buf[..len])))
}

/// `james@acme.example` on a domain-joined or Entra-joined PC; `None` on a
/// local account.
pub(crate) fn user_principal_name() -> Option<String> {
    let mut size: u32 = 0;
    // SAFETY: a null buffer asks for the size, in characters.
    unsafe { GetUserNameExW(NameUserPrincipal, std::ptr::null_mut(), &mut size) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u16; size as usize];
    // SAFETY: the buffer holds `size` characters, as passed.
    let ok = unsafe { GetUserNameExW(NameUserPrincipal, buf.as_mut_ptr(), &mut size) };
    if !ok {
        return None;
    }
    let name = String::from_utf16_lossy(&buf[..size as usize]);
    (!name.is_empty()).then_some(name)
}
