//! Windows: the control pipe for this user alone. The pipe's name is
//! predictable (`\\.\pipe\anvil-<user>`), so the server grants access to
//! its own user and nobody else, and a client talks only to a server
//! running as itself — another user on the machine (a shared PC, Remote
//! Desktop) can neither drive the phone nor pose as it.

use std::io;
use std::ptr::{null_mut, NonNull};

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// A handle closed when dropped.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// The user `process` runs as, as a SID string (`S-1-5-21-…`).
fn user_of(process: HANDLE) -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: `token` receives a handle we close.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Owned(token);
    let mut needed = 0u32;
    // SAFETY: asks the size first; the second call fills a buffer that big.
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed) };
    // u64s, so the TOKEN_USER read from it is aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the buffer holds a TOKEN_USER.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text: *mut u16 = null_mut();
    // SAFETY: `text` is allocated by Windows and freed with LocalFree.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    let string = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    unsafe { LocalFree(text.cast()) };
    Ok(string)
}

/// The user this process runs as.
pub fn this_user() -> io::Result<String> {
    // SAFETY: the pseudo-handle needs no closing.
    user_of(unsafe { GetCurrentProcess() })
}

/// Security attributes that let this user, and nobody else, open the pipe.
pub struct UserOnly {
    descriptor: NonNull<core::ffi::c_void>,
    attributes: SECURITY_ATTRIBUTES,
}

// The descriptor is read-only once made.
unsafe impl Send for UserOnly {}
unsafe impl Sync for UserOnly {}

impl UserOnly {
    pub fn new() -> io::Result<Self> {
        // Protected (no inherited entries), full access for this user only.
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", this_user()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `descriptor` is allocated by Windows, freed on drop.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let descriptor = NonNull::new(descriptor).ok_or_else(io::Error::last_os_error)?;
        Ok(Self {
            descriptor,
            attributes: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.as_ptr(),
                bInheritHandle: 0,
            },
        })
    }

    /// An instance of the pipe `name` that this user alone may open;
    /// `first` refuses a name someone already serves.
    pub fn create(
        &mut self,
        name: &str,
        first: bool,
    ) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        let attributes: *mut SECURITY_ATTRIBUTES = &mut self.attributes;
        // SAFETY: the attributes and their descriptor outlive the call.
        unsafe {
            tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, attributes.cast())
        }
    }
}

impl Drop for UserOnly {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.descriptor.as_ptr()) };
    }
}

/// Whether the pipe `client` is connected to is served by this user.
pub fn served_by_this_user(client: &impl std::os::windows::io::AsRawHandle) -> io::Result<bool> {
    let mut pid = 0u32;
    // SAFETY: `client` is a connected pipe handle.
    if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as HANDLE, &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: closed by `Owned`.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Owned(process);
    Ok(user_of(process.0)? == this_user()?)
}
