//! The half of the program that has no window: a tray icon and a global hotkey.
//!
//! The widget is resident. It starts hidden, costs a notification-area icon, and
//! appears when the hotkey is pressed — so the panel is a thing you summon rather
//! than a window you keep somewhere.
//!
//! # Why a thread of its own, and why it does the showing
//!
//! Because the alternative cannot work. An egui application only runs code inside a
//! frame, and a hidden window is painted no frames at all: there is no `WM_PAINT`
//! for a window with no visible region, so [`egui::ViewportCommand`] has nowhere to
//! run and `ctx.request_repaint` has nothing to repaint. A hidden widget that waits
//! for its own paint loop to notice the hotkey waits for ever.
//!
//! So this module owns a plain Win32 thread with its own hidden top-level window and
//! its own `GetMessage` loop, which keeps running whether the panel is visible or
//! not. It receives `WM_HOTKEY` and the tray icon's callbacks, and it shows and hides
//! the panel *itself*, with `ShowWindow` on the panel's `HWND` — the one mechanism
//! that works from either side. The panel publishes that handle to [`Handle`] as it
//! is constructed; everything else the two halves need to agree on is an atomic.
//!
//! ```text
//! resident thread                 egui thread
//! ───────────────                 ───────────
//! WM_HOTKEY ─┐            place ─→ ShowWindow ─→ WM_PAINT ─→ App::ui
//! tray click ┼─→ toggle ─→ shows += 1               ↑          │ focus lost, and
//! menu Show ─┘            wake                      └──────────┘ not staying open
//!                                                     remember   ShowWindow(SW_HIDE)
//! menu Quit  ─→ WM_CLOSE ─→ (winit CloseRequested, eframe returns)
//! ```
//!
//! The hidden window is top-level rather than message-only (`HWND_MESSAGE`) for one
//! reason: `TaskbarCreated` is a broadcast, and broadcasts do not reach message-only
//! windows. Without it, restarting Explorer would take the icon away for good.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, DeleteObject, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowRect,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetWindowPos, ShowWindow, TrackPopupMenu, TranslateMessage, HICON, HWND_TOP, ICONINFO,
    MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, SW_HIDE, SW_SHOW, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_DESTROY,
    WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};

use crate::audio::Wake;
use crate::hotkey::Hotkey;

/// The tray icon's callback message. Anything from `WM_APP` up is ours to define.
const WM_TRAY: u32 = WM_APP + 1;
/// Our own message asking the thread to take the icon down and stop.
const WM_QUIT_RESIDENT: u32 = WM_APP + 2;

const HOTKEY_ID: i32 = 1;
const ICON_ID: u32 = 1;

const MENU_SHOW: usize = 1;
const MENU_QUIT: usize = 2;

/// How far the panel's top-left lands from the pointer when it is summoned.
const CURSOR_OFFSET: i32 = 12;

/// Where the panel goes when it is summoned.
///
/// Both answers begin from the monitor the pointer is on, because a widget summoned by
/// a keystroke has to arrive in front of the person who pressed it. They differ only in
/// where *on* that monitor it lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// Under the pointer, wherever that is — the transient popup.
    AtPointer,
    /// Where it was left, measured from its monitor's work area rather than from the
    /// desktop, so "the same place" means the same place on *this* screen. A panel
    /// parked in the bottom-right corner is in the bottom-right corner of whichever
    /// monitor the pointer is on.
    LastPosition,
}

/// What the two halves of the program share.
struct State {
    /// The panel's `HWND` as an integer, or 0 before [`Handle::adopt_panel`] has run.
    /// The resident thread does nothing at all until this is set.
    panel: AtomicI64,
    /// The resident thread's own hidden window, for [`Resident::shutdown`].
    resident: AtomicI64,
    /// Incremented on every show. The panel watches it to know it is being shown
    /// again, which is the only way it can tell a fresh appearance from the frame
    /// after a blur — see `App::hide_on_blur`.
    shows: AtomicU64,
    /// Whether the panel is up, as far as this program is concerned.
    ///
    /// Ours rather than `IsWindowVisible`'s, because between startup and the first
    /// summon the two disagree: eframe reveals the window itself after its first
    /// frame, so Windows calls it visible while it is parked off the desktop and no
    /// user has asked for anything. Asking Win32 would make the first hotkey press
    /// *hide* the widget.
    visible: AtomicBool,
    /// Whether the panel has ever been put somewhere a user can see.
    ///
    /// [`Placement::LastPosition`] has no last position until there has been one, and
    /// the only position the window has before that is [`crate::PARKED`] — which is not
    /// a position anyone gave it. So the first summon always goes to the pointer,
    /// whatever the mode says.
    placed: AtomicBool,
    /// Where the next summon puts the panel. [`Placement::LastPosition`] as `true`.
    at_pointer: AtomicBool,
    /// The last position, as an offset from the work-area corner of the monitor the
    /// panel was on — not as a desktop coordinate.
    ///
    /// Two monitors rarely share an origin, a size or a scale, so a remembered desktop
    /// position is a remembered *monitor*, which is exactly what it must not be. Taken
    /// on every hide, so dragging the panel and putting it away remembers the drag.
    last_offset: (AtomicI32, AtomicI32),
    /// Set while the panel is visible; the audio backend polls only then.
    wake: Arc<Wake>,
}

/// The panel's end of the resident thread. Cheap to clone.
#[derive(Clone)]
pub struct Handle(Arc<State>);

impl Handle {
    /// Tell the resident thread which window to show, and adopt nothing else.
    ///
    /// Called once, from the panel's constructor rather than its first frame,
    /// because there is no first frame until something shows the window and nothing
    /// can show it until this has run.
    pub fn adopt_panel(&self, hwnd: isize) {
        self.0.panel.store(hwnd as i64, Ordering::Release);
    }

    /// The flag the audio backend sleeps on. Follows visibility.
    pub fn wake(&self) -> Arc<Wake> {
        self.0.wake.clone()
    }

    /// How many times the panel has been shown. Changes mean "shown again".
    pub fn shows(&self) -> u64 {
        self.0.shows.load(Ordering::Acquire)
    }

    /// Keep the resident side's copy of the placement mode in step with the panel's.
    ///
    /// The resident thread is what moves the window, so this is the only setting of the
    /// two chrome toggles it needs to know about: staying open on a blur is decided in
    /// the panel's own frame, where the focus is visible.
    pub fn set_placement(&self, placement: Placement) {
        self.0
            .at_pointer
            .store(placement == Placement::AtPointer, Ordering::Release);
    }

    pub fn show(&self) {
        show(&self.0);
    }

    pub fn hide(&self) {
        hide(&self.0);
    }
}

/// The resident thread, owned by `main` for as long as the program runs.
pub struct Resident {
    handle: Handle,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Resident {
    /// Spawn the thread, register the hotkey and add the tray icon.
    ///
    /// Returns the [`Handle`] whatever happens on the far side: a machine where the
    /// hotkey is already taken, or where the notification area refuses the icon,
    /// still runs the widget — it is only the ways *in* that are missing, and those
    /// are reported where the user will see them rather than swallowed.
    pub fn spawn(hotkey: Hotkey, placement: Placement) -> Self {
        let state = Arc::new(State {
            panel: AtomicI64::new(0),
            resident: AtomicI64::new(0),
            shows: AtomicU64::new(0),
            visible: AtomicBool::new(false),
            placed: AtomicBool::new(false),
            at_pointer: AtomicBool::new(placement == Placement::AtPointer),
            last_offset: (AtomicI32::new(0), AtomicI32::new(0)),
            wake: Arc::new(Wake::new(false)),
        });

        let thread_state = state.clone();
        let thread = std::thread::Builder::new()
            .name("resident".into())
            .spawn(move || run(thread_state, hotkey))
            .expect("spawn resident thread");

        Self {
            handle: Handle(state),
            thread: Some(thread),
        }
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Take the icon out of the notification area and stop the thread.
    ///
    /// Called after `run_native` returns, which is the one moment at which the
    /// program is certainly on its way out and the thread is certainly still there.
    /// Skipping it leaves a ghost icon behind until the user waves the pointer over
    /// it, because the shell only notices an owner has gone when it next tries to
    /// talk to it.
    pub fn shutdown(&mut self) {
        let resident = HWND(self.handle.0.resident.load(Ordering::Acquire) as *mut _);
        if !resident.0.is_null() {
            unsafe {
                let _ = PostMessageW(Some(resident), WM_QUIT_RESIDENT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Showing and hiding
// ---------------------------------------------------------------------------

fn panel(state: &State) -> Option<HWND> {
    let raw = state.panel.load(Ordering::Acquire);
    if raw == 0 {
        return None;
    }
    Some(HWND(raw as *mut core::ffi::c_void))
}

/// Show the panel, put it in the foreground, and start the audio poll.
///
/// The order matters at the end: `shows` is incremented *before* the window can
/// paint, so the first frame after an appearance already knows it is a new one.
fn show(state: &State) {
    let Some(hwnd) = panel(state) else {
        return;
    };
    unsafe {
        // Always placed, never merely revealed — both modes are relative to the monitor
        // the pointer is on, and the pointer has usually moved since last time. The
        // first summon goes to the pointer whatever the mode says: there is no last
        // position yet, only [`crate::PARKED`].
        if state.at_pointer.load(Ordering::Acquire) || !state.placed.load(Ordering::Acquire) {
            place_at_cursor(hwnd);
        } else {
            place_at_last_offset(hwnd, state);
        }
        state.placed.store(true, Ordering::Release);
        state.visible.store(true, Ordering::Release);
        state.shows.fetch_add(1, Ordering::Release);
        state.wake.set(true);
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Allowed here where it usually is not: the process that received the last
        // input event may take the foreground, and a hotkey press is that event.
        let _ = SetForegroundWindow(hwnd);
    }
}

fn hide(state: &State) {
    let Some(hwnd) = panel(state) else {
        return;
    };
    // Before the window goes, while it still has a position worth remembering — and on
    // every hide rather than only when the mode asks for it, so that turning
    // [`Placement::LastPosition`] on does not need a position it never took.
    //
    // Only if it was actually up, though: the panel hides itself on its very first frame
    // if nobody has summoned it, and what it would be remembering there is
    // [`crate::PARKED`], 30,000px off the corner of the desktop.
    if state.visible.load(Ordering::Acquire) {
        unsafe {
            remember_offset(hwnd, state);
        }
    }
    state.visible.store(false, Ordering::Release);
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    state.wake.set(false);
}

fn toggle(state: &State) {
    if state.visible.load(Ordering::Acquire) {
        hide(state);
    } else {
        show(state);
    }
}

// ---------------------------------------------------------------------------
// Placing it
// ---------------------------------------------------------------------------

/// Where the panel is about to go: the pointer, the usable rectangle of the monitor it
/// is on, and the panel's own size.
///
/// One reading of all three, because both modes need all three and asking twice for the
/// pointer between the two would let it move in between.
struct Target {
    cursor: POINT,
    /// The work area — the monitor minus the taskbar, so the panel cannot land behind
    /// it.
    work: RECT,
    width: i32,
    height: i32,
}

unsafe fn target(hwnd: HWND) -> Option<Target> {
    let mut cursor = POINT::default();
    GetCursorPos(&mut cursor).ok()?;
    let mut rect = RECT::default();
    GetWindowRect(hwnd, &mut rect).ok()?;
    // `MONITOR_DEFAULTTONEAREST`, so a pointer in the seam between two monitors resolves
    // to one of them rather than to nothing.
    let work = work_area(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST))?;
    Some(Target {
        cursor,
        work,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    })
}

unsafe fn work_area(monitor: HMONITOR) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        Some(info.rcWork)
    } else {
        None
    }
}

/// [`Placement::AtPointer`]: under the pointer, a little down and to the right.
///
/// The transient popup — it belongs where the user is looking, which for something
/// summoned from the keyboard while the pointer sits over whatever is being turned down
/// is at the pointer. Also the first summon in either mode, because until then the only
/// position the window has is [`crate::PARKED`].
unsafe fn place_at_cursor(hwnd: HWND) {
    let Some(target) = target(hwnd) else {
        return;
    };
    move_into_view(
        hwnd,
        target.cursor.x + CURSOR_OFFSET,
        target.cursor.y + CURSOR_OFFSET,
        &target,
    );
}

/// [`Placement::LastPosition`]: where it was left, on the screen the pointer is on.
///
/// The offset is measured from a work-area corner rather than from the desktop origin,
/// which is the whole of what makes this useful on more than one monitor: a panel parked
/// in the bottom-right corner comes back in the bottom-right corner of whichever screen
/// the pointer is on, instead of coming back on a screen nobody is looking at.
///
/// Between monitors of different scale the arrival is approximate: the clamp uses the
/// size the window has *now*, and winit resizes it for the new scale factor a frame
/// later. It is off by the difference, in the corner cases, and self-corrects on the
/// next hide — which is a better trade than not opening on the right screen.
unsafe fn place_at_last_offset(hwnd: HWND, state: &State) {
    let Some(target) = target(hwnd) else {
        return;
    };
    let x = target.work.left + state.last_offset.0.load(Ordering::Acquire);
    let y = target.work.top + state.last_offset.1.load(Ordering::Acquire);
    move_into_view(hwnd, x, y, &target);
}

/// Take note of where the panel is, so [`Placement::LastPosition`] has somewhere to put
/// it next time.
unsafe fn remember_offset(hwnd: HWND, state: &State) {
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return;
    }
    let Some(work) = work_area(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)) else {
        return;
    };
    state
        .last_offset
        .0
        .store(rect.left - work.left, Ordering::Release);
    state
        .last_offset
        .1
        .store(rect.top - work.top, Ordering::Release);
}

/// Move the panel to `x, y`, clamped so that all of it is inside the work area.
unsafe fn move_into_view(hwnd: HWND, x: i32, y: i32, target: &Target) {
    let work = target.work;
    let x = x.clamp(work.left, (work.right - target.width).max(work.left));
    let y = y.clamp(work.top, (work.bottom - target.height).max(work.top));
    // NOACTIVATE, because the caller grants the foreground a moment later and doing
    // it here as well makes the window flash at its old position first.
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOP),
        x,
        y,
        0,
        0,
        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

// ---------------------------------------------------------------------------
// The thread
// ---------------------------------------------------------------------------

// What the window procedure needs, reachable from a `static extern "system" fn`.
//
// A thread local rather than `GWLP_USERDATA`: the procedure only ever runs on this
// thread, dispatched from the loop below, so there is nothing a pointer smuggled
// through the window would buy that this does not.
thread_local! {
    static CONTEXT: std::cell::RefCell<Option<Context>> = const { std::cell::RefCell::new(None) };
}

struct Context {
    state: Arc<State>,
    /// `TaskbarCreated`, resolved at run time — Explorer broadcasts it when it
    /// restarts, and an icon added before that is gone unless it is added again.
    taskbar_created: u32,
    icon: HICON,
    tip: Vec<u16>,
}

fn run(state: Arc<State>, hotkey: Hotkey) {
    unsafe {
        let class = WNDCLASSW {
            lpszClassName: w!("azur-audio-switcher-resident"),
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            eprintln!("resident: could not register a window class; no tray icon or hotkey");
            return;
        }

        // Never shown, but top-level: only top-level windows receive the
        // `TaskbarCreated` broadcast. `WS_EX_TOOLWINDOW` keeps it out of Alt-Tab in
        // case anything ever does make it visible.
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("azur-audio-switcher-resident"),
            w!("Azur Audio Switcher (resident)"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        ) else {
            eprintln!("resident: could not create the resident window; no tray icon or hotkey");
            return;
        };
        state.resident.store(hwnd.0 as i64, Ordering::Release);

        let icon = tray_icon();
        let tip: Vec<u16> = format!("{} — {} to open", crate::NAME, hotkey.label)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        CONTEXT.with(|slot| {
            *slot.borrow_mut() = Some(Context {
                state: state.clone(),
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
                icon,
                tip: tip.clone(),
            });
        });

        let added = add_icon(hwnd, icon, &tip);

        // Both ways in are reported the same way — through the icon if there is one,
        // and to the console either way, since a `windows` subsystem build has no
        // console in release and this is the sort of thing a user runs from one
        // exactly once to find out why nothing happens.
        let registered = RegisterHotKey(Some(hwnd), HOTKEY_ID, hotkey.modifiers, hotkey.key as u32);
        match (&registered, added) {
            (Ok(()), true) => balloon(
                hwnd,
                &format!("{} is running", crate::NAME),
                &format!("Press {} to open the panel.", hotkey.label),
            ),
            (Ok(()), false) => {
                eprintln!(
                    "resident: the notification area refused the icon; the hotkey still works"
                )
            }
            (Err(_), true) => {
                eprintln!(
                    "resident: {} is already taken by another program",
                    hotkey.label
                );
                balloon(
                    hwnd,
                    "Hotkey unavailable",
                    &format!(
                        "{} is already taken. Click here to open the panel, or restart with --hotkey to choose another.",
                        hotkey.label
                    ),
                );
            }
            (Err(_), false) => eprintln!(
                "resident: no tray icon and {} is taken — nothing can open the panel",
                hotkey.label
            ),
        }

        let mut message = MSG::default();
        loop {
            // `GetMessage` has three outcomes, not two: nonzero for a message, zero
            // for `WM_QUIT`, and -1 for an error — which `as_bool` reads as *true*,
            // so a bare `while` on it spins for ever on a dead queue.
            let result = GetMessageW(&mut message, None, 0, 0);
            if result.0 <= 0 {
                if result.0 == -1 {
                    eprintln!(
                        "resident: the message loop failed; no tray icon or hotkey from here"
                    );
                }
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        if registered.is_ok() {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
        }
        remove_icon(hwnd);
        if !icon.is_invalid() {
            let _ = DestroyIcon(icon);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Borrowed and dropped before anything that could re-enter: `TrackPopupMenu`
    // pumps messages, and a message arriving while the cell was still borrowed would
    // panic rather than open a menu.
    let borrowed = CONTEXT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|c| (c.state.clone(), c.taskbar_created))
    });
    let Some((state, taskbar_created)) = borrowed else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };

    match msg {
        WM_HOTKEY => {
            toggle(&state);
            LRESULT(0)
        }
        WM_TRAY => {
            // Classic tray semantics, which is what not calling `NIM_SETVERSION`
            // asks for: the low word of `lParam` is the mouse message.
            match (lparam.0 as u32) & 0xffff {
                WM_LBUTTONUP => toggle(&state),
                WM_RBUTTONUP => unsafe { context_menu(hwnd, &state) },
                _ => {}
            }
            LRESULT(0)
        }
        WM_QUIT_RESIDENT => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ if msg == taskbar_created => {
            // Explorer restarted and forgot us. Everything else survived, because
            // the window, the hotkey and the icon handle are all ours — so this is
            // the one branch that needs the icon and its tooltip again.
            CONTEXT.with(|slot| {
                if let Some(context) = slot.borrow().as_ref() {
                    unsafe { add_icon(hwnd, context.icon, &context.tip) };
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Ask the panel's own event loop to end, which ends the program.
///
/// `WM_CLOSE` rather than anything of ours: winit turns it into `CloseRequested`,
/// eframe leaves its loop, `run_native` returns, and `main` gets to take the icon
/// down and run the panel's own shutdown. A hidden window still receives it —
/// messages are dispatched whether or not there is anything to paint, which is the
/// asymmetry this whole module is built around.
fn quit_program(state: &State) {
    match panel(state) {
        Some(hwnd) => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        // No panel to ask, so there is nothing running but this thread.
        None => std::process::exit(0),
    }
}

unsafe fn context_menu(hwnd: HWND, state: &State) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_SHOW,
        if state.visible.load(Ordering::Acquire) {
            w!("Hide")
        } else {
            w!("Show")
        },
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));

    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    // The documented dance: the menu needs the foreground to dismiss itself when the
    // user clicks elsewhere, and `TPM_RETURNCMD` keeps the choice here rather than
    // in a `WM_COMMAND` that would arrive after the menu is gone.
    let _ = SetForegroundWindow(hwnd);
    let choice = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD,
        cursor.x,
        cursor.y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    match choice.0 as usize {
        MENU_SHOW => toggle(state),
        MENU_QUIT => quit_program(state),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// The notification area
// ---------------------------------------------------------------------------

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    }
}

unsafe fn add_icon(hwnd: HWND, icon: HICON, tip: &[u16]) -> bool {
    let mut data = notify_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    let len = tip.len().min(data.szTip.len());
    data.szTip[..len].copy_from_slice(&tip[..len]);
    data.szTip[data.szTip.len() - 1] = 0;
    Shell_NotifyIconW(NIM_ADD, &data).as_bool()
}

unsafe fn remove_icon(hwnd: HWND) {
    let data = notify_data(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
}

/// A one-off balloon, which is how a program with no window says something.
fn balloon(hwnd: HWND, title: &str, body: &str) {
    let mut data = notify_data(hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    write_utf16(&mut data.szInfoTitle, title);
    write_utf16(&mut data.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn write_utf16(field: &mut [u16], text: &str) {
    let encoded: Vec<u16> = text.encode_utf16().take(field.len() - 1).collect();
    field[..encoded.len()].copy_from_slice(&encoded);
    field[encoded.len()] = 0;
}

// ---------------------------------------------------------------------------
// The icon itself
// ---------------------------------------------------------------------------

/// Build the tray icon: the application's mark, at the size the shell asks for.
///
/// Not the system's own volume icon, which `SndVolSSO.dll` would have handed over
/// for two lines of code — because that speaker is already sitting in the notification
/// area three places along, and an icon a user cannot tell from Windows' own is worse
/// than no icon at all. The mark is three faders on a plate for that reason among
/// others: whatever the speaker tucked under them says, it is not any of the glyphs
/// the shell already puts in this strip.
///
/// [`crate::mark`] instead, which is the same art the executable carries in its
/// resources. The plate is what makes it legible here rather than a decoration: the
/// notification area is a light taskbar on one machine, a dark one on the next, and a
/// transparent one over a photograph on the third, and an icon cannot switch. That is
/// the reason Azur's brief puts the mark on a plate instead of shipping bare ink.
///
/// `SM_CXSMICON` follows the display scale, so this is 16px at 100% and 24 at 150% —
/// and because `assets/` holds a bitmap per size rather than one to scale, the 16 is a
/// 16 that was drawn as one rather than a shrunken 256. The size comes back from
/// [`crate::mark::bitmap`] because a metric between two of them rounds up.
fn tray_icon() -> HICON {
    let wanted =
        unsafe { GetSystemMetrics(SM_CXSMICON).max(GetSystemMetrics(SM_CYSMICON)) }.max(16) as u32;
    let (size, art) = crate::mark::bitmap(wanted);
    let pixels = premultiplied_bgra(art);

    unsafe {
        // A 32bpp BGRA colour bitmap with a full alpha channel, and an empty mask —
        // which is how a modern alpha icon is spelled: `CreateIconIndirect` takes the
        // mask as significant only for 1bpp and 24bpp bitmaps.
        let colour = CreateBitmap(
            size as i32,
            size as i32,
            1,
            32,
            Some(pixels.as_ptr().cast()),
        );
        if colour.is_invalid() {
            return HICON::default();
        }
        // Zeroed, and explicitly so: `CreateBitmap` with null bits leaves them
        // uninitialised, and the documentation only says the mask is *ignored* for a
        // 32bpp colour bitmap — a promise thin enough that handing it a screenful of
        // whatever was on the heap would be a bug that shows up as an icon with holes
        // in it on somebody else's machine. Zero in an AND mask means "use the
        // colour pixel", which is what the alpha channel is already saying.
        let stride = size.div_ceil(16) * 2;
        let empty = vec![0u8; (stride * size) as usize];
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, Some(empty.as_ptr().cast()));
        let info = ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: colour,
            ..Default::default()
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        // `CreateIconIndirect` copies both bitmaps, so they are ours to free — and a
        // tray icon that leaks two bitmaps a restart is the sort of thing that is
        // invisible for weeks.
        let _ = DeleteObject(colour.into());
        if !mask.is_invalid() {
            let _ = DeleteObject(mask.into());
        }
        icon
    }
}

/// Straight RGBA as the premultiplied BGRA that GDI wants.
///
/// Two conversions in one pass, because they are the same pass: Windows bitmaps are
/// blue-first, and `CreateIconIndirect` composites an alpha icon expecting the colour
/// channels to have been multiplied through already. Skipping the multiply is the
/// classic way an icon ends up with a bright halo on a dark taskbar.
fn premultiplied_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        let alpha = pixel[3] as u32;
        let through = |channel: u8| ((channel as u32 * alpha + 127) / 255) as u8;
        out.push(through(pixel[2])); // blue
        out.push(through(pixel[1])); // green
        out.push(through(pixel[0])); // red
        out.push(pixel[3]);
    }
    out
}
