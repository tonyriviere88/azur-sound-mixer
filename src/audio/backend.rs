//! The Core Audio backend: MMDevice for the endpoints, WASAPI session control
//! for the applications.
//!
//! Runs entirely on the backend thread ([`super::run`]), which owns the COM
//! apartment these interfaces are bound to. Nothing here is `Send`, and nothing
//! here needs to be.
//!
//! # What is cached and why
//!
//! Interface pointers, aggressively. Activating `IAudioEndpointVolume` on a
//! device or building an `IAudioSessionEnumerator` are the expensive calls;
//! reading a volume or a peak level off an interface you already hold is a read
//! out of the audio engine's shared memory. So the two enumerations run on slow
//! clocks and the poll only ever touches interfaces that are already bound.
//!
//! The cache is invalidated by identity, not by time: an endpoint is rebound when
//! the default device's id changes, and a session is dropped when its instance
//! identifier stops appearing in the enumeration.

use std::collections::HashMap;
use std::sync::Arc;

use windows::core::{Interface, Result, HRESULT, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, AudioSessionStateActive,
    AudioSessionStateExpired, EDataFlow, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL, STGM_READ};

use super::policy::{IPolicyConfig, CPOLICY_CONFIG_CLIENT};
use super::process::{self, IconImage};
use super::wstr::{take_com_string, wide};
use super::{Command, Device, Endpoint, Flow, Session, Snapshot};

/// A default endpoint this backend currently holds interfaces for.
///
/// Everything an endpoint can be asked, in one value — so that replacing the
/// default device is a single assignment and cannot leave half the interfaces
/// pointing at the device that just stopped being it.
struct Bound {
    /// The device id the interfaces below belong to. When the system's default
    /// stops matching this, they are stale.
    id: String,
    volume: IAudioEndpointVolume,
    meter: Option<IAudioMeterInformation>,
    /// The session manager — on a render endpoint only. The volume mixer is about
    /// what is being played, so capture endpoints do not get one.
    sessions: Option<IAudioSessionManager2>,
}

/// One application session, with its interfaces already resolved.
struct Live {
    key: String,
    name: String,
    pid: u32,
    is_system: bool,
    control: IAudioSessionControl2,
    volume: ISimpleAudioVolume,
    meter: Option<IAudioMeterInformation>,
    icon: Option<Arc<IconImage>>,
}

/// A process's name and icon, resolved once per executable.
#[derive(Clone)]
struct Identity {
    name: String,
    icon: Option<Arc<IconImage>>,
}

pub struct Backend {
    enumerator: IMMDeviceEnumerator,
    /// `Err` when `CPolicyConfigClient` would not hand over [`IPolicyConfig`] —
    /// device switching is then unavailable, and the rest of the widget still
    /// works. The `HRESULT` is kept rather than discarded: this interface is
    /// undocumented enough that "which way it failed" is the whole diagnosis, and
    /// `E_NOINTERFACE` (the IID moved) reads nothing like `REGDB_E_CLASSNOTREG`
    /// (no audio stack) once it reaches the panel.
    ///
    /// Spelled out because `Result` in this module is Win32's one-parameter alias.
    policy: std::result::Result<IPolicyConfig, HRESULT>,

    outputs: Vec<Device>,
    inputs: Vec<Device>,
    render: Option<Bound>,
    capture: Option<Bound>,

    sessions: Vec<Live>,
    identities: HashMap<String, Identity>,
    /// Sticky: once device switching has failed, say so rather than silently
    /// doing nothing every time the user picks a device.
    error: Option<String>,
}

impl Backend {
    pub fn new() -> Result<Self> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        // Undocumented, so its absence is a missing feature rather than a failure
        // to start.
        let policy: std::result::Result<IPolicyConfig, HRESULT> =
            unsafe { CoCreateInstance(&CPOLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }
                .map_err(|error| error.code());

        let mut backend = Self {
            enumerator,
            policy,
            outputs: Vec::new(),
            inputs: Vec::new(),
            render: None,
            capture: None,
            sessions: Vec::new(),
            identities: HashMap::new(),
            error: None,
        };
        backend.rescan_devices();
        backend.rescan_sessions();
        Ok(backend)
    }

    // -----------------------------------------------------------------------
    // Enumeration
    // -----------------------------------------------------------------------

    /// Re-read both device lists and both defaults, rebinding whatever moved.
    pub fn rescan_devices(&mut self) {
        self.outputs = self.list(eRender);
        self.inputs = self.list(eCapture);

        let default_render = self.default_id(eRender);
        let default_capture = self.default_id(eCapture);

        if self.render.as_ref().map(|b| &b.id) != default_render.as_ref() {
            // Bind first, then swap: the sessions being dropped are the old
            // device's, and the new endpoint carries the manager that replaces
            // the enumerator they came from.
            let bound = default_render.and_then(|id| self.bind(eRender, id));
            self.sessions.clear();
            self.render = bound;
        }
        if self.capture.as_ref().map(|b| &b.id) != default_capture.as_ref() {
            self.capture = default_capture.and_then(|id| self.bind(eCapture, id));
        }
    }

    fn list(&self, flow: EDataFlow) -> Vec<Device> {
        unsafe {
            // Active only: `DEVICE_STATE_UNPLUGGED` and `DEVICE_STATE_DISABLED`
            // devices are what the sound page hides behind "show disconnected
            // devices", and offering one here would fail on selection.
            let Ok(collection) = self
                .enumerator
                .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
            else {
                return Vec::new();
            };
            let count = collection.GetCount().unwrap_or(0);
            (0..count)
                .filter_map(|i| collection.Item(i).ok())
                .filter_map(|device| {
                    Some(Device {
                        id: device_id(&device)?,
                        name: device_name(&device),
                    })
                })
                .collect()
        }
    }

    fn default_id(&self, flow: EDataFlow) -> Option<String> {
        unsafe {
            let device = self
                .enumerator
                .GetDefaultAudioEndpoint(flow, eConsole)
                .ok()?;
            device_id(&device)
        }
    }

    /// Activate the interfaces for one endpoint.
    ///
    /// The meter is optional on purpose: a device whose driver does not implement
    /// `IAudioMeterInformation` still has a perfectly good volume control, and
    /// losing the level display is not a reason to show the row as broken.
    fn bind(&self, flow: EDataFlow, id: String) -> Option<Bound> {
        unsafe {
            let key = wide(&id);
            let device = self.enumerator.GetDevice(PCWSTR(key.as_ptr())).ok()?;
            // The volume is the one interface an endpoint is useless without.
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
            let meter: Option<IAudioMeterInformation> = device.Activate(CLSCTX_ALL, None).ok();
            let sessions = (flow == eRender)
                .then(|| {
                    device
                        .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                        .ok()
                })
                .flatten();
            Some(Bound {
                id,
                volume,
                meter,
                sessions,
            })
        }
    }

    /// Rebuild the session list off the default output device.
    ///
    /// `IAudioSessionEnumerator` is a snapshot taken at call time, so this is the
    /// only way to notice a new application — but the sessions it hands back are
    /// matched against the ones already held by instance identifier, so a row
    /// that was already there keeps its resolved name and icon rather than
    /// re-reading them off disk every rescan.
    pub fn rescan_sessions(&mut self) {
        let Some(manager) = self.render.as_ref().and_then(|b| b.sessions.clone()) else {
            self.sessions.clear();
            return;
        };
        let mut previous: HashMap<String, Live> = std::mem::take(&mut self.sessions)
            .into_iter()
            .map(|live| (live.key.clone(), live))
            .collect();

        unsafe {
            let Ok(enumerator) = manager.GetSessionEnumerator() else {
                return;
            };
            let count = enumerator.GetCount().unwrap_or(0);
            for i in 0..count {
                let Ok(session) = enumerator.GetSession(i) else {
                    continue;
                };
                let Ok(control) = session.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                // Expired means the process is gone and only the bookkeeping is
                // left. Inactive is kept: a paused video's volume is still worth
                // showing, which is what Windows' own mixer does. A state that
                // cannot be read is treated as expired — a session that will not
                // answer is not one to put a slider on.
                if control.GetState().unwrap_or(AudioSessionStateExpired)
                    == AudioSessionStateExpired
                {
                    continue;
                }
                let Some(key) =
                    take_com_string(control.GetSessionInstanceIdentifier().unwrap_or_default())
                else {
                    continue;
                };
                if let Some(live) = previous.remove(&key) {
                    self.sessions.push(live);
                    continue;
                }
                let Ok(volume) = session.cast::<ISimpleAudioVolume>() else {
                    continue;
                };
                let is_system = control.IsSystemSoundsSession() == S_OK;
                let pid = control.GetProcessId().unwrap_or(0);
                let identity = self.identify(&control, pid, is_system);
                self.sessions.push(Live {
                    key,
                    name: identity.name,
                    pid,
                    is_system,
                    control,
                    volume,
                    meter: session.cast().ok(),
                    icon: identity.icon,
                });
            }
        }

        // A deterministic base order: alphabetical, with the pid as the tiebreak so
        // two instances of the same program hold their places across rescans.
        //
        // Only the base. The order the panel *shows* — starred, then active, then
        // the rest — is the UI's, because two thirds of that key is something the
        // backend has no opinion about: which applications the user has starred is
        // not audio state at all.
        self.sessions.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.pid.cmp(&b.pid))
        });
    }

    /// Work out what to call a session, and what to draw beside it.
    fn identify(&mut self, control: &IAudioSessionControl2, pid: u32, is_system: bool) -> Identity {
        if is_system {
            return Identity {
                name: "System sounds".to_owned(),
                icon: None,
            };
        }

        let path = process::executable_path(pid);
        if let Some(path) = &path {
            if let Some(cached) = self.identities.get(path) {
                return cached.clone();
            }
        }

        // A session's own display name, when it set one. Most do not, and those
        // that do sometimes set an indirect resource string (`@dll,-id`) meant to
        // be run through `SHLoadIndirectString` — not worth resolving, since the
        // executable's own description is the better name anyway.
        let declared = unsafe { take_com_string(control.GetDisplayName().unwrap_or_default()) }
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty() && !name.starts_with('@'));

        let identity = match &path {
            Some(path) => Identity {
                name: declared.unwrap_or_else(|| process::display_name(path)),
                icon: process::icon(path).map(Arc::new),
            },
            None => Identity {
                name: declared.unwrap_or_else(|| format!("Process {pid}")),
                icon: None,
            },
        };
        if let Some(path) = path {
            self.identities.insert(path, identity.clone());
        }
        identity
    }

    // -----------------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------------

    /// Read every value the UI shows off interfaces already held.
    pub fn poll(&mut self) -> Snapshot {
        let output = read_endpoint(self.render.as_ref());
        let input = read_endpoint(self.capture.as_ref());

        let sessions = self
            .sessions
            .iter()
            .map(|live| unsafe {
                Session {
                    key: live.key.clone(),
                    name: live.name.clone(),
                    pid: live.pid,
                    is_system: live.is_system,
                    active: live
                        .control
                        .GetState()
                        .is_ok_and(|s| s == AudioSessionStateActive),
                    volume: live.volume.GetMasterVolume().unwrap_or(0.0),
                    muted: live.volume.GetMute().map(|m| m.as_bool()).unwrap_or(false),
                    peak: live
                        .meter
                        .as_ref()
                        .and_then(|m| m.GetPeakValue().ok())
                        .unwrap_or(0.0),
                    icon: live.icon.clone(),
                }
            })
            .collect();

        let index_of = |devices: &[Device], bound: Option<&Bound>| {
            let id = &bound?.id;
            devices.iter().position(|d| &d.id == id)
        };
        Snapshot {
            default_output: index_of(&self.outputs, self.render.as_ref()),
            default_input: index_of(&self.inputs, self.capture.as_ref()),
            outputs: self.outputs.clone(),
            inputs: self.inputs.clone(),
            output,
            input,
            sessions,
            error: self.error.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // Writing
    // -----------------------------------------------------------------------

    pub fn apply(&mut self, command: &Command) {
        // Every write is best-effort: the device can be pulled out between the
        // poll that produced the row and the click that acted on it, and the next
        // rescan is what corrects the display.
        unsafe {
            match command {
                Command::SetEndpointVolume(flow, value) => {
                    if let Some(bound) = self.endpoint(*flow) {
                        let _ = bound
                            .volume
                            .SetMasterVolumeLevelScalar(value.clamp(0.0, 1.0), std::ptr::null());
                    }
                }
                Command::SetEndpointMute(flow, muted) => {
                    if let Some(bound) = self.endpoint(*flow) {
                        let _ = bound.volume.SetMute(*muted, std::ptr::null());
                    }
                }
                Command::SetSessionVolume(key, value) => {
                    if let Some(live) = self.session(key) {
                        let _ = live
                            .volume
                            .SetMasterVolume(value.clamp(0.0, 1.0), std::ptr::null());
                    }
                }
                Command::SetSessionMute(key, muted) => {
                    if let Some(live) = self.session(key) {
                        let _ = live.volume.SetMute(*muted, std::ptr::null());
                    }
                }
                Command::SetDefaultDevice(id) => self.set_default(id),
            }
        }
    }

    /// Make a device the default for every role.
    ///
    /// All three, because that is what picking a device on the Windows sound page
    /// does: leaving `eCommunications` behind gives you a machine whose media
    /// plays out of the new device while calls stay on the old one, which reads as
    /// the switch not having worked.
    fn set_default(&mut self, id: &str) {
        let policy = match &self.policy {
            Ok(policy) => policy.clone(),
            Err(hr) => {
                self.error = Some(format!(
                    "Switching devices needs IPolicyConfig, which this system did not provide ({hr:?})."
                ));
                return;
            }
        };
        let key = wide(id);
        let device = PCWSTR(key.as_ptr());
        for role in [eConsole, eMultimedia, eCommunications] {
            let hr = unsafe { policy.SetDefaultEndpoint(device, role) };
            if hr.is_err() {
                self.error = Some(format!("Could not set the default device ({hr:?})."));
                return;
            }
        }
        // Only the system's idea of the default has moved; the caller re-enumerates,
        // which is what rebinds every interface that hung off the old one.
        self.error = None;
    }

    fn endpoint(&self, flow: Flow) -> Option<&Bound> {
        match flow {
            Flow::Output => self.render.as_ref(),
            Flow::Input => self.capture.as_ref(),
        }
    }

    fn session(&self, key: &str) -> Option<&Live> {
        self.sessions.iter().find(|live| live.key == key)
    }
}

fn read_endpoint(bound: Option<&Bound>) -> Endpoint {
    let Some(bound) = bound else {
        return Endpoint::default();
    };
    unsafe {
        Endpoint {
            volume: bound.volume.GetMasterVolumeLevelScalar().unwrap_or(0.0),
            muted: bound.volume.GetMute().map(|m| m.as_bool()).unwrap_or(false),
            peak: bound
                .meter
                .as_ref()
                .and_then(|m| m.GetPeakValue().ok())
                .unwrap_or(0.0),
            present: true,
        }
    }
}

unsafe fn device_id(device: &IMMDevice) -> Option<String> {
    take_com_string(device.GetId().unwrap_or_default())
}

/// `PKEY_Device_FriendlyName`: the "Speakers (Realtek(R) Audio)" form, which is
/// the device *and* its adapter — the only form that distinguishes two identical
/// headsets on different ports.
unsafe fn device_name(device: &IMMDevice) -> String {
    device
        .OpenPropertyStore(STGM_READ)
        .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
        .map(|value| value.to_string())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Unknown device".to_owned())
}
