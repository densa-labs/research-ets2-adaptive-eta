#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};

#[derive(Debug)]
pub(crate) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub(crate) fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(Self(handle))
    }

    pub(crate) const fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `OwnedHandle` is constructed only from a valid owned Windows
        // handle and closes it exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) fn error_code() -> u32 {
    // SAFETY: `GetLastError` has no preconditions.
    unsafe { GetLastError() }
}

pub(crate) fn last_error() -> std::io::Error {
    error_from_code(error_code())
}

pub(crate) fn error_from_code(code: u32) -> std::io::Error {
    std::io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX))
}
