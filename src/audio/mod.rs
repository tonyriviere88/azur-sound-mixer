//! The audio model, and the thread that keeps it in step with the system.
//!
//! Everything the UI draws comes out of one [`Snapshot`], and everything it does
//! goes in as one [`Command`]. Nothing above this module knows that Core Audio
//! exists.
//!
//! # Why a thread
//!
//! Core Audio is COM, and COM calls block: enumerating endpoints talks to the
//! audio service, and a device that is being reconfigured can take tens of
//! milliseconds to answer. Doing that from the paint loop drops frames while the
//! user is dragging a slider — exactly when it is most visible.
//!
//! So the backend lives on its own MTA thread and publishes immutable snapshots.
//! The UI reads the latest one under a mutex it holds for the length of an
//! `Arc::clone`, and the thread asks for a repaint when it publishes, which is
//! what makes an external volume change (a keyboard media key, another mixer)
//! show up here without polling from the UI side.

mod backend;
mod policy;
mod process;
mod wstr;

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub use process::IconImage;

/// Which end of the machine a device or endpoint is on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Flow {
    /// Speakers, headphones — anything sound comes out of.
    Output,
    /// Microphones and line inputs.
    Input,
}

/// Which job a default endpoint is the default *for*.
///
/// Windows keeps three roles per flow — `eConsole`, `eMultimedia` and
/// `eCommunications` — and this keeps two, because only one of those boundaries is a
/// boundary anyone can hear. Nothing in the shipping Windows UI separates console from
/// multimedia; the sound page moves them together and so does this. Communications is
/// the one that is genuinely a different device on a real machine: the headset you take
/// calls on while music stays on the speakers.
///
/// So [`Role::Default`] is "where sound goes" and [`Role::Communications`] is "where a
/// call goes". The two are **disjoint**: neither writes the other's role, which is what
/// makes the panel's two device sections independent of each other rather than one being
/// an override the other can undo. The mapping onto Windows' three lives in one place,
/// `backend::Backend::set_default`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    /// Console and multimedia together: every ordinary sound on the machine.
    Default,
    /// Communications — what a voice call opens, and what the Windows sound page
    /// calls the *default communication device*.
    Communications,
}

/// One endpoint device the user could pick as their default.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Device {
    /// The MMDevice id — opaque, stable across reboots, and what
    /// `IPolicyConfig::SetDefaultEndpoint` takes.
    pub id: String,
    /// `PKEY_Device_FriendlyName`: "Speakers (Realtek(R) Audio)".
    pub name: String,
}

/// The master volume of one end of the machine.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Endpoint {
    /// `0.0..=1.0`, on Core Audio's scalar (perceptual) curve rather than dB —
    /// which is the one the Windows volume slider itself moves along, so a value
    /// shown here matches the one shown there.
    pub volume: f32,
    pub muted: bool,
    /// Current output level, `0.0..=1.0`. Zero when nothing is playing.
    pub peak: f32,
    /// False when there is no such device at all — an unplugged headset, a
    /// machine with no microphone.
    pub present: bool,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            volume: 0.0,
            muted: false,
            peak: 0.0,
            present: false,
        }
    }
}

/// One application's slice of the default output device.
#[derive(Clone, Debug)]
pub struct Session {
    /// The session *instance* identifier: unique per process, where the plain
    /// session identifier is shared by every instance of the same executable.
    /// This is the row's identity across refreshes, so two Firefox windows keep
    /// their own sliders.
    pub key: String,
    /// What to call it: the session's own display name if it set one, else the
    /// executable's `FileDescription`, else the file stem.
    pub name: String,
    pub pid: u32,
    /// The `System sounds` pseudo-session, which has no process behind it.
    pub is_system: bool,
    /// Whether the session is currently rendering. Inactive sessions are still
    /// listed — a browser tab that has finished a video keeps its volume — but
    /// the UI dims them.
    pub active: bool,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
    /// The executable's icon, decoded once per path and shared by every session
    /// of that process.
    pub icon: Option<Arc<IconImage>>,
}

/// Everything the widget draws, as of one instant.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    /// Index into `outputs` / `inputs` of the current default, if there is one.
    pub default_output: Option<usize>,
    pub default_input: Option<usize>,
    /// The same two indices, into the same two lists, for [`Role::Communications`].
    ///
    /// Separate fields rather than a map keyed by [`Role`]: there are two roles and
    /// they are not interchangeable — everything else in this snapshot belongs to the
    /// [`Role::Default`] endpoints, because they are the ones the sliders, the meters
    /// and the session list hang off. These two are the only thing this widget knows
    /// about the communications endpoints, and knowing which device it is is the whole
    /// of what the panel needs.
    pub communications_output: Option<usize>,
    pub communications_input: Option<usize>,
    pub output: Endpoint,
    pub input: Endpoint,
    /// Sessions on the default output device, in a deterministic base order —
    /// alphabetical, tie-broken by pid. The order they are *shown* in is the UI's,
    /// since it groups by whether the user starred them.
    pub sessions: Vec<Session>,
    /// Set when the backend could not start or lost the audio service. The UI
    /// shows it instead of pretending the sliders work.
    pub error: Option<String>,
}

/// What the UI asks the backend to do.
///
/// Every variant is a write, so they all begin `Set` — which clippy reads as a
/// redundant prefix behind `Command::`, and which is the point: `Command::Session‑
/// Volume(..)` would read as a question where `Command::SetSessionVolume(..)` reads
/// as the instruction it is. This enum has no queries; snapshots answer those.
#[derive(Clone, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum Command {
    SetEndpointVolume(Flow, f32),
    SetEndpointMute(Flow, bool),
    /// Make this device the default for one [`Role`].
    ///
    /// One role's endpoints and no other's: [`Role::Default`] writes `eConsole` and
    /// `eMultimedia`, [`Role::Communications`] writes `eCommunications`. The Windows
    /// sound page moves all three at once when you pick a device on it; this panel has a
    /// control per role instead, so a device chosen for calls is not quietly replaced the
    /// next time the one for sound changes.
    ///
    /// No [`Flow`]: a device id already belongs to one, and the backend answers
    /// "which endpoint moved" by re-reading the defaults rather than by being
    /// told. This is the one command that invalidates its caches — everything
    /// downstream of a default endpoint, its volume interface, its meter and every
    /// session on it, belongs to the device that just stopped being the default.
    SetDefaultDevice(Role, String),
    SetSessionVolume(String, f32),
    SetSessionMute(String, bool),
}

/// How often volumes, mutes and peak levels are re-read.
///
/// 20Hz: fast enough that a peak meter reads as a level rather than a blink,
/// slow enough that a dozen sessions cost nothing measurable. These are in-proc
/// calls into the audio engine's shared memory, not round trips to the service.
const POLL: Duration = Duration::from_millis(50);

/// How often the session list itself is rebuilt.
///
/// Rebuilding means a new `IAudioSessionEnumerator` — a real allocation and a
/// walk of every session on the device — so it runs an order of magnitude less
/// often than the value poll. An application that starts playing shows up within
/// this long; anything already listed updates at [`POLL`].
const RESCAN: Duration = Duration::from_millis(600);

/// How often the device lists and the default endpoints are re-read.
const RESCAN_DEVICES: Duration = Duration::from_millis(1500);

/// How long the backend sleeps between checks while the widget is hidden.
///
/// Nothing depends on this being short. The panel appearing does not wait for it —
/// [`Wake::set`] signals the condition variable and the backend is polling within
/// microseconds — so all this bounds is how long a shutdown takes to notice that the
/// UI has dropped the channel.
const ASLEEP: Duration = Duration::from_millis(500);

/// Whether the backend should be reading the audio engine at all.
///
/// A resident widget is hidden almost all of the time, and a hidden widget is drawn
/// no frames — so a backend that kept its 20Hz poll would be a program that costs a
/// COM call every 50ms for ever in order to compute values nobody is looking at.
/// This is the flag that stops it, set by whichever side shows or hides the panel.
///
/// A condition variable rather than a bare [`AtomicBool`](std::sync::atomic::AtomicBool):
/// the flag is polled on the way *down*, where a late answer costs nothing, but on the
/// way up it is the first thing between the hotkey and a panel with live values in
/// it, and there the backend has to start immediately rather than at the end of
/// whatever sleep it happened to be in.
pub struct Wake {
    awake: Mutex<bool>,
    changed: Condvar,
}

impl Wake {
    pub fn new(awake: bool) -> Self {
        Self {
            awake: Mutex::new(awake),
            changed: Condvar::new(),
        }
    }

    pub fn set(&self, awake: bool) {
        *self.awake.lock().expect("wake mutex") = awake;
        self.changed.notify_all();
    }

    pub fn is_awake(&self) -> bool {
        *self.awake.lock().expect("wake mutex")
    }

    /// Block until awake or until `timeout` elapses, and report which.
    fn wait(&self, timeout: Duration) -> bool {
        let guard = self.awake.lock().expect("wake mutex");
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, timeout, |awake| !*awake)
            .expect("wake mutex");
        *guard
    }
}

/// The handle the UI keeps: a channel out, and the latest snapshot in.
pub struct AudioController {
    commands: Sender<Command>,
    latest: Arc<Mutex<Arc<Snapshot>>>,
}

impl AudioController {
    /// Start the backend thread.
    ///
    /// `ctx` is repainted whenever a new snapshot lands, so the widget follows
    /// volume changes made anywhere else on the system. `wake` is what stops it
    /// while the widget is hidden — see [`Wake`].
    pub fn spawn(ctx: egui::Context, wake: Arc<Wake>) -> Self {
        let (commands, rx) = std::sync::mpsc::channel();
        let latest: Arc<Mutex<Arc<Snapshot>>> = Arc::new(Mutex::new(Arc::default()));

        let published = latest.clone();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(rx, published, ctx, wake))
            .expect("spawn audio thread");

        Self { commands, latest }
    }

    /// The most recent snapshot. Cheap — one `Arc` clone under the lock.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.latest.lock().expect("snapshot mutex").clone()
    }

    /// Queue a command. Dropped silently if the backend thread has died, which
    /// is also the case in which the snapshot carries an error for the UI to
    /// show.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// The backend thread's whole life.
///
/// One loop, three clocks: commands are applied as they arrive, values are
/// re-read every [`POLL`], and the two enumerations run on their own slower
/// beats. A command that changes what is being enumerated resets those clocks
/// rather than waiting them out. Above all three sits [`Wake`]: while the widget is
/// hidden the loop does nothing at all, and the first turn after it is shown re-reads
/// the world rather than trusting anything it learned before going to sleep.
fn run(
    commands: Receiver<Command>,
    latest: Arc<Mutex<Arc<Snapshot>>>,
    ctx: egui::Context,
    wake: Arc<Wake>,
) {
    let _com = wstr::ComGuard::new_multithreaded();

    let mut backend = match backend::Backend::new() {
        Ok(backend) => backend,
        Err(error) => {
            publish(
                &latest,
                &ctx,
                Snapshot {
                    error: Some(format!("Core Audio unavailable: {error}")),
                    ..Default::default()
                },
            );
            return;
        }
    };

    let mut next_poll = Instant::now();
    let mut next_rescan = Instant::now();
    let mut next_device_rescan = Instant::now();

    loop {
        if !wake.is_awake() {
            // Hidden. Commands are still applied — a write that was in flight when
            // the panel went away should still land — but nothing is read, because
            // nothing is being drawn.
            match commands.try_recv() {
                Ok(command) => {
                    backend.apply(&command);
                    continue;
                }
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => {}
            }
            if wake.wait(ASLEEP) {
                // Shown again, after however long. Every clock is reset rather than
                // resumed: the session list, the device list and the default
                // endpoints are all things that change while a widget is hidden, and
                // the panel's first frame should show what is true now, not what was
                // true when it was last looked at.
                let now = Instant::now();
                next_poll = now;
                next_rescan = now;
                next_device_rescan = now;
            }
            continue;
        }

        // Wait for the next command, but never past the next poll. `Disconnected`
        // means the UI is gone.
        let wait = next_poll.saturating_duration_since(Instant::now());
        match commands.recv_timeout(wait) {
            Ok(command) => {
                let invalidates = matches!(command, Command::SetDefaultDevice(..));
                backend.apply(&command);
                if invalidates {
                    // The default endpoint moved, so the session list, both
                    // volume interfaces and the device lists all belong to the
                    // old device. Re-read everything on the next turn.
                    //
                    // Both roles, though only [`Role::Default`] can have invalidated
                    // anything: nothing hangs off the communications endpoints, so for
                    // those this is not an invalidation but a confirmation — it is what
                    // gets the picker its answer on the next turn instead of up to
                    // `RESCAN_DEVICES` later. Telling the two apart would buy one
                    // skipped enumeration on a control nobody touches twice a minute.
                    next_rescan = Instant::now();
                    next_device_rescan = Instant::now();
                }
                // Publish immediately so the control the user just moved does not
                // wait up to POLL to confirm.
                next_poll = Instant::now();
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let now = Instant::now();
        next_poll = now + POLL;

        if now >= next_device_rescan {
            next_device_rescan = now + RESCAN_DEVICES;
            backend.rescan_devices();
        }
        if now >= next_rescan {
            next_rescan = now + RESCAN;
            backend.rescan_sessions();
        }

        publish(&latest, &ctx, backend.poll());
    }
}

fn publish(latest: &Mutex<Arc<Snapshot>>, ctx: &egui::Context, snapshot: Snapshot) {
    *latest.lock().expect("snapshot mutex") = Arc::new(snapshot);
    ctx.request_repaint();
}
