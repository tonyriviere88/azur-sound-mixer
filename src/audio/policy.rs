//! `IPolicyConfig` — how a default audio endpoint is actually set.
//!
//! Windows has no public API for "make this the default device". The MMDevice
//! API can only read the current default ([`IMMDeviceEnumerator::GetDefaultAudioEndpoint`]);
//! the setter lives on an undocumented interface that the sound control panel
//! and the Settings app use, and which every third-party device switcher on
//! Windows has used since Vista.
//!
//! That means three things worth being explicit about:
//!
//! - **The vtable order is the contract.** COM dispatches by slot, so the ten
//!   methods before `SetDefaultEndpoint` have to be declared even though nothing
//!   here calls them — a missing one silently shifts every later slot and calls
//!   the wrong function. They are declared with opaque pointer arguments, since
//!   their real signatures involve `WAVEFORMATEX` and share-mode structures this
//!   crate has no use for.
//! - **The IID is not the one most write-ups give.** Two revisions of this
//!   interface have circulated for as long as it has been used, both named
//!   `IPolicyConfig` and both paired with the same coclass:
//!   `f8679f50-850a-455c-9d37-1cba05e2c3b0`, which the C# switchers picked up,
//!   and `f8679f50-850a-41cf-9c72-430f290290c8`, which the C/C++ ones did. Only
//!   the second is still answered — on Windows 11 26200 the first returns
//!   `E_NOINTERFACE`, and its bytes do not appear in `AudioSes.dll` at all. The
//!   surviving one is what Xbox Game Bar, `mmsys.cpl` and the volume flyout all
//!   ask that coclass for. The method list below is the same for both, so this
//!   is a GUID difference and nothing more.
//! - **It can disappear.** Nothing documents this, so every call site treats a
//!   failed `CoCreateInstance` as "device switching is unavailable" and leaves
//!   the rest of the widget working, rather than refusing to start. The failing
//!   `HRESULT` is kept and shown, because "not provided" and "provided under a
//!   different IID" are the same sentence otherwise.
//!
//! [`IMMDeviceEnumerator::GetDefaultAudioEndpoint`]: windows::Win32::Media::Audio::IMMDeviceEnumerator::GetDefaultAudioEndpoint

// COM method names, kept exactly as the interface declares them: this file is
// checked by reading it against the vtable it describes, and renaming a slot to
// suit Rust's casing would be the one change that cannot be caught by a compiler.
#![allow(non_snake_case)]

use windows::core::{interface, IUnknown, IUnknown_Vtbl, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::ERole;

/// `CPolicyConfigClient`, the coclass behind the interface.
pub const CPOLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
pub unsafe trait IPolicyConfig: IUnknown {
    // ---- Slots 0..9: placeholders, present only to keep the layout ----
    fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut core::ffi::c_void) -> HRESULT;
    fn GetDeviceFormat(
        &self,
        device: PCWSTR,
        default: i32,
        format: *mut *mut core::ffi::c_void,
    ) -> HRESULT;
    fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
    fn SetDeviceFormat(
        &self,
        device: PCWSTR,
        endpoint_format: *mut core::ffi::c_void,
        mix_format: *mut core::ffi::c_void,
    ) -> HRESULT;
    fn GetProcessingPeriod(
        &self,
        device: PCWSTR,
        default: i32,
        default_period: *mut i64,
        min_period: *mut i64,
    ) -> HRESULT;
    fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
    fn GetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    fn SetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    fn GetPropertyValue(
        &self,
        device: PCWSTR,
        key: *const PROPERTYKEY,
        value: *mut core::ffi::c_void,
    ) -> HRESULT;
    fn SetPropertyValue(
        &self,
        device: PCWSTR,
        key: *const PROPERTYKEY,
        value: *mut core::ffi::c_void,
    ) -> HRESULT;

    // ---- Slot 10: the one this crate exists to reach ----
    /// Make `device` the default endpoint for one role.
    ///
    /// `device` is an `IMMDevice::GetId` string. The roles are independent, so
    /// switching a device the way the Settings app does means calling this once
    /// per role.
    ///
    /// The only method here that is public: the placeholders above exist to hold
    /// the vtable's shape, and calling one of them with the opaque arguments they
    /// are declared with would be undefined behaviour.
    pub fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;

    /// Slot 11, unused — declared so the interface is complete rather than cut
    /// off at the method that happened to be needed.
    fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::wstr::ComGuard;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

    /// The coclass still answers for the IID above.
    ///
    /// Worth a test of its own because the failure is silent in the way that
    /// matters: the widget starts, every volume works, and only picking a device
    /// does nothing. Whenever it does start failing, this says whether the
    /// coclass went away (`REGDB_E_CLASSNOTREG`) or the interface moved again
    /// (`E_NOINTERFACE`) — which are the same symptom and different fixes.
    #[test]
    fn the_coclass_still_answers_for_this_iid() {
        let _com = ComGuard::new_multithreaded();
        let policy =
            unsafe { CoCreateInstance::<_, IPolicyConfig>(&CPOLICY_CONFIG_CLIENT, None, CLSCTX_ALL) };
        assert!(
            policy.is_ok(),
            "CPolicyConfigClient did not hand over IPolicyConfig: {:?}. \
             E_NOINTERFACE means this build revised the IID again — see the module docs.",
            policy.err().map(|error| error.code())
        );
    }
}
