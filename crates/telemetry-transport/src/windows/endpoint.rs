#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::handle::OwnedHandle;

const PIPE_PREFIX: &str = r"\\.\pipe\DensaLabs.AdaptiveETA.Telemetry.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pipe_name: String,
    pipe_name_wide: Vec<u16>,
    user_sid: String,
    session_id: u32,
}

#[derive(Debug)]
pub enum EndpointError {
    InvalidIdentity(&'static str),
    Io(io::Error),
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentity(detail) => {
                write!(formatter, "invalid Windows identity: {detail}")
            }
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EndpointError {}

impl From<io::Error> for EndpointError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.pipe_name.fmt(formatter)
    }
}

impl Endpoint {
    /// Derives the deterministic current-user/current-session pipe endpoint.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error if the process identity cannot be read.
    pub fn for_current_user() -> Result<Self, EndpointError> {
        // SAFETY: `GetCurrentProcess` returns a process pseudo-handle valid for
        // token queries in this process.
        let user_sid = unsafe { sid_for_process(GetCurrentProcess())? };
        let session_id = current_session_id()?;
        Self::for_identity(&user_sid, session_id)
    }

    #[cfg(test)]
    pub(crate) fn for_test(suffix: &str) -> Result<Self, EndpointError> {
        // SAFETY: `GetCurrentProcess` returns a process pseudo-handle valid for
        // token queries in this process.
        let user_sid = unsafe { sid_for_process(GetCurrentProcess())? };
        let session_id = current_session_id()?;
        let mut endpoint = Self::for_identity(&user_sid, session_id)?;
        if suffix.is_empty()
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(EndpointError::InvalidIdentity("invalid test suffix"));
        }
        endpoint.pipe_name.push_str(".test-");
        endpoint.pipe_name.push_str(suffix);
        if endpoint.pipe_name.encode_utf16().count() >= 256 {
            return Err(EndpointError::InvalidIdentity("pipe name is too long"));
        }
        endpoint.pipe_name_wide = wide_null(&endpoint.pipe_name);
        Ok(endpoint)
    }

    fn for_identity(user_sid: &str, session_id: u32) -> Result<Self, EndpointError> {
        if user_sid.is_empty()
            || !user_sid
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'-' || byte == b'S')
        {
            return Err(EndpointError::InvalidIdentity("unexpected SID text"));
        }
        let pipe_name = format!(r"{PIPE_PREFIX}.{user_sid}.session-{session_id}");
        if pipe_name.encode_utf16().count() >= 256 {
            return Err(EndpointError::InvalidIdentity("pipe name is too long"));
        }
        let pipe_name_wide = wide_null(&pipe_name);
        Ok(Self {
            pipe_name,
            pipe_name_wide,
            user_sid: user_sid.to_owned(),
            session_id,
        })
    }

    pub(crate) fn pipe_name_ptr(&self) -> *const u16 {
        self.pipe_name_wide.as_ptr()
    }

    pub(crate) fn security_descriptor(&self) -> Result<SecurityDescriptor, EndpointError> {
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", self.user_sid);
        SecurityDescriptor::from_sddl(&sddl).map_err(Into::into)
    }

    pub(crate) fn validate_server(&self, pipe: HANDLE) -> Result<(), EndpointError> {
        let mut process_id = 0_u32;
        // SAFETY: `pipe` is a connected named-pipe client handle and the output
        // pointer is valid for one `u32`.
        if unsafe { GetNamedPipeServerProcessId(pipe, &raw mut process_id) } == 0 {
            return Err(last_error().into());
        }
        let mut session_id = 0_u32;
        // SAFETY: `process_id` came from the kernel and the output pointer is valid.
        if unsafe { ProcessIdToSessionId(process_id, &raw mut session_id) } == 0 {
            return Err(last_error().into());
        }
        if session_id != self.session_id {
            return Err(EndpointError::InvalidIdentity(
                "named-pipe server is in another session",
            ));
        }
        // SAFETY: opening a process for a token identity query does not transfer
        // memory ownership; the returned handle is wrapped immediately.
        let process = OwnedHandle::new(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id)
        })
        .ok_or_else(last_error)?;
        // SAFETY: `process` remains valid for the duration of the token query.
        let server_sid = unsafe { sid_for_process(process.raw())? };
        if server_sid != self.user_sid {
            return Err(EndpointError::InvalidIdentity(
                "named-pipe server is owned by another user",
            ));
        }
        Ok(())
    }
}

pub(crate) struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let sddl = wide_null(sddl);
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: the input is a NUL-terminated UTF-16 string; Windows allocates
        // the returned self-relative descriptor, which `Drop` releases.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(Self(descriptor))
    }

    pub(crate) fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
                .expect("SECURITY_ATTRIBUTES fits in u32"),
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by the matching Windows conversion API.
        unsafe {
            let _ = LocalFree(self.0.cast::<c_void>());
        }
    }
}

fn current_session_id() -> io::Result<u32> {
    let mut session_id = 0_u32;
    // SAFETY: the current process ID is valid and the output pointer is valid.
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &raw mut session_id) } == 0 {
        Err(last_error())
    } else {
        Ok(session_id)
    }
}

unsafe fn sid_for_process(process: HANDLE) -> io::Result<String> {
    let mut token = std::ptr::null_mut();
    // SAFETY: the caller provides a valid process handle; `token` is a valid output.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(last_error());
    }
    let token = OwnedHandle::new(token).ok_or_else(last_error)?;
    let mut required = 0_u32;
    // SAFETY: the null-buffer probe is the documented sizing operation.
    unsafe {
        let _ = GetTokenInformation(
            token.raw(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &raw mut required,
        );
    }
    if required == 0 {
        return Err(last_error());
    }
    let required_usize = usize::try_from(required).expect("u32 fits usize");
    let word_size = std::mem::size_of::<usize>();
    let mut buffer = vec![0_usize; required_usize.div_ceil(word_size)];
    // SAFETY: `buffer` has the exact size requested by the preceding API call.
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &raw mut required,
        )
    } == 0
    {
        return Err(last_error());
    }
    // SAFETY: successful `TokenUser` output begins with a valid `TOKEN_USER`.
    let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid_text = std::ptr::null_mut();
    // SAFETY: the SID pointer belongs to the still-live token information buffer.
    if unsafe { ConvertSidToStringSidW(token_user.User.Sid, &raw mut sid_text) } == 0 {
        return Err(last_error());
    }
    let result = wide_ptr_to_string(sid_text);
    // SAFETY: the SID string was allocated by `ConvertSidToStringSidW`.
    unsafe {
        let _ = LocalFree(sid_text.cast::<c_void>());
    }
    result
}

fn wide_ptr_to_string(pointer: *const u16) -> io::Result<String> {
    if pointer.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned a null SID string",
        ));
    }
    let mut length = 0_usize;
    // SAFETY: Windows returned a NUL-terminated SID string.
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` was determined by scanning the same NUL-terminated allocation.
    String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SID was not valid UTF-16"))
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn last_error() -> io::Error {
    // SAFETY: `GetLastError` has no preconditions.
    io::Error::from_raw_os_error(i32::try_from(unsafe { GetLastError() }).unwrap_or(i32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_name_is_stable_and_scoped_by_user_and_session() {
        let first = Endpoint::for_identity("S-1-5-21-1000", 3).expect("endpoint");
        let same = Endpoint::for_identity("S-1-5-21-1000", 3).expect("same endpoint");
        let other_user = Endpoint::for_identity("S-1-5-21-2000", 3).expect("other user");
        let other_session = Endpoint::for_identity("S-1-5-21-1000", 4).expect("other session");
        assert_eq!(first, same);
        assert_ne!(first, other_user);
        assert_ne!(first, other_session);
        assert_eq!(
            first.to_string(),
            r"\\.\pipe\DensaLabs.AdaptiveETA.Telemetry.v1.S-1-5-21-1000.session-3"
        );
    }

    #[test]
    fn endpoint_name_rejects_non_sid_text() {
        assert!(Endpoint::for_identity(r"S-1-5-21\other", 1).is_err());
        assert!(Endpoint::for_identity("", 1).is_err());
    }
}
