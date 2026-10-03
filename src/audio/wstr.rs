//! The three string shapes Win32 hands back, and an apartment guard.

use windows::core::PWSTR;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};

/// Hold COM open for as long as this lives.
///
/// The backend thread is multithreaded-apartment: it makes no window and pumps
/// no messages, and MTA is the apartment Core Audio's own worker threads run in,
/// so calls go straight through instead of being marshalled to a message pump
/// that does not exist.
pub struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    pub fn new_multithreaded() -> Self {
        // Already-initialized is not an error for us: it means someone else on
        // this thread got there first and owns the uninitialize.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

/// A NUL-terminated UTF-16 buffer to pass into Win32.
///
/// The returned `Vec` has to outlive the call that borrows its pointer, so bind
/// it to a name rather than taking `.as_ptr()` on a temporary.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read a UTF-16 slice up to its first NUL, if any.
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Read an out-parameter string and free it.
///
/// Core Audio returns `PWSTR`s allocated with `CoTaskMemAlloc` — `GetId`,
/// `GetDisplayName`, `GetSessionInstanceIdentifier` — and the caller owns every
/// one of them. Taking the value and the free in a single step is what keeps
/// that from being a leak per poll.
///
/// # Safety
///
/// `p` must be null or a `CoTaskMemAlloc`-allocated NUL-terminated string that
/// this call is allowed to free.
pub unsafe fn take_com_string(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let value = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    value
}
