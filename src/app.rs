//! The widget itself: one panel, five sections, top to bottom.
//!
//! ```text
//! ┌──────────────────────────────────────┐
//! │ 🎚 Audio                    ⧉ 🖱 📌 ✕ │  drag; then sound settings, where it
//! ├──────────────────────────────────────┤  opens, stay open, hide
//! │ 🔊  42 ──────●────────────────        │  master output; the glyph mutes
//! │ 🎤 100 ─────────────────────●         │  master input;  the glyph mutes
//! │ ──────────────────────────────────── │
//! │ Default devices                      │
//! │ [ Speakers (Realtek(R) Audio)    ˅ ] │
//! │ [ Microphone Array               ˅ ] │
//! │ ──────────────────────────────────── │
//! │ Communications                    ˅  │  where calls go; shut until opened
//! │ ──────────────────────────────────── │
//! │ Volume mixer                         │
//! │ ▣ System sounds              🔊  ☆   │  one block per session,
//! │   ────────────●───────────────────   │  scrolling
//! │ ▣ Firefox                    🔊  ★   │
//! │   ──────────────────●─────────────   │
//! └──────────────────────────────────────┘
//! ```
//!
//! Every mute in the panel is the same control: a speaker or microphone glyph that
//! toggles, red when it is off. Two of them are the machine's endpoints, the rest
//! are applications.
//!
//! # Where the state lives
//!
//! Nowhere, mostly. Every value drawn is read out of the latest
//! [`Snapshot`] each frame, so the widget agrees with the
//! system rather than with itself — press a keyboard volume key and this moves.
//!
//! The exceptions are the three things the system cannot tell us: which
//! applications are starred, how the panel itself is set up — the two chrome toggles and
//! which section is open — and the couple of hundred milliseconds after a write during
//! which the backend has not read the new value back yet. That last one is [`Echo`], and
//! it is what keeps a dragged slider from stuttering against a 20Hz poll.
//!
//! The first two outlive the process — [`crate::favourites`] and [`crate::settings`],
//! one file each under `%APPDATA%` — because both of them are the user telling the
//! widget how they want it, and asking twice is asking too often.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use azur_egui_theme::components::{
    collection_label, Divider, IconButton, Select, Size, Slider, Text, TextColor, TextVariant,
    Variant,
};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::space;
use azur_egui_theme::Theme;
use egui::{
    pos2, vec2, Align, Color32, Layout, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2,
    ViewportCommand,
};

use crate::audio::{AudioController, Command, Endpoint, Flow, IconImage, Role, Session, Snapshot};
use crate::favourites::Favourites;
use crate::icons;
use crate::resident;
use crate::settings::Settings;
use crate::system;
use crate::widgets;

/// How long a locally-set value is trusted over the backend's reading.
///
/// Long enough to cover a poll interval and the round trip through the audio
/// engine, short enough that a write which silently failed corrects itself before
/// the user has moved on.
const ECHO: Duration = Duration::from_millis(400);

/// The same, for a default-device switch — which takes visibly longer, because
/// every session on the machine is being moved to another endpoint.
const ECHO_DEVICE: Duration = Duration::from_millis(2000);

/// The gap between two blocks in the mixer list.
const MIXER_GAP: f32 = space::S3;

/// What the user did to a master row this frame.
///
/// A row now carries two controls — the slider and the mute glyph — and `masters`
/// owns the state both of them write to, so the row reports rather than acts.
#[derive(Default)]
struct RowEdit {
    /// A new volume, `0..=1`.
    volume: Option<f32>,
    /// The glyph was clicked.
    toggle_mute: bool,
}

/// How far in from an edge the pointer starts a resize.
///
/// The content is inset by `space-3`, so a 6px reach never overlaps anything the
/// user could want to click — including the mixer's scrollbar, which sits inside
/// that margin rather than against the glass.
const GRIP: f32 = 6.0;

/// The square at each corner, which resizes both axes at once.
const CORNER: f32 = 14.0;

/// The mark in the header, in logical pixels, inside a box of [`MARK_BOX`].
///
/// 16, which is Azur's Part A size for a glyph in a row of them: it sits beside the
/// close and pin buttons and has to weigh the same as they do.
const MARK: f32 = 16.0;

/// What the mark is allocated, which is a little more than it draws — the box is the
/// title's leading edge and the drag region's, and the mark is centred in it.
const MARK_BOX: f32 = 18.0;

pub struct App {
    theme: Theme,
    audio: AudioController,
    favourites: Favourites,
    /// The resident half of the program: the tray icon, the hotkey, and the one
    /// place that shows and hides this window.
    resident: resident::Handle,
    /// Whether the panel stays up when it loses the focus, and stays above other
    /// windows. One of the two chrome toggles; see [`App::hide_on_blur`].
    stay_open: bool,
    /// Where the panel appears when it is summoned. The other toggle, and independent of
    /// the first: staying open is about *when* it goes away, this is about *where* it
    /// comes back.
    placement: resident::Placement,
    /// The same two values as they are on disk, which is not necessarily as they are
    /// above: a command-line flag overrides one for this run without replacing what is
    /// remembered. Written when the user clicks a toggle, and only then — see
    /// [`crate::settings`].
    settings: Settings,

    /// Uploaded application icons, keyed by the address of the `Arc` the backend
    /// shares — which is stable for as long as any snapshot still refers to the
    /// process, and gone from every snapshot once it exits.
    textures: HashMap<usize, TextureHandle>,
    /// This program's own mark in the header, and which of [`crate::mark`]'s sizes it
    /// was uploaded from. Kept because the display scale can change under a running
    /// window — dragging it to a second monitor is enough — and the answer is then a
    /// different bitmap rather than the same one stretched.
    mark: Option<(u32, TextureHandle)>,
    /// Smoothed meter levels, by session key. Swept alongside the textures.
    peaks: HashMap<String, f32>,
    echo: Echo,
    /// Which `Select` the pointer last changed, so the pick can be sent once
    /// rather than every frame it stays open.
    output_pick: Option<usize>,
    input_pick: Option<usize>,
    /// The same, for the communications section's two. Four in all, because a `Select`
    /// writes its pick into a place its caller owns and there are four of them — and
    /// because a communications pick and a default pick are different answers to the
    /// same list, so one field could not hold both.
    communications_output_pick: Option<usize>,
    communications_input_pick: Option<usize>,
    /// Whether the window has held the input focus since it was last shown. See
    /// [`App::hide_on_blur`].
    focus_seen: bool,
    /// Whether it had the focus on the *previous* frame, which is the other half of what
    /// `focus_seen` cannot say: one is "ever, since being shown", this is "last frame", and
    /// the difference between them is a blur — the edge, not the level. See
    /// [`App::close_stale_popups`].
    was_focused: bool,
    /// The value of [`resident::Handle::shows`] this frame's focus tracking belongs
    /// to. A different one means the panel has been summoned again since.
    shows_seen: u64,
    /// Whether the communications section is showing its two pickers.
    ///
    /// Shut until somebody opens it, and then remembered — the third thing in
    /// [`crate::settings`], written on the same click that draws it. The section exists at
    /// all because a shut one is affordable: where calls go is a thing somebody sets when
    /// they get a headset and then leaves alone for months, so it earns a row rather than
    /// the 80px two more selects would take out of the mixer list.
    ///
    /// The argument against remembering it was that a disclosure which remembers being
    /// opened is a section that is never shut again. The answer is that shutting it is
    /// remembered too: what comes back is the state it was last *left* in, which for
    /// somebody who never touches it is shut, and for somebody who runs two devices is the
    /// pair of pickers they went looking for the last time.
    communications_open: bool,
}

impl App {
    /// Build the panel, and hand its window to the resident thread.
    ///
    /// `start_visible` is the `--show` flag: without it the widget comes up hidden,
    /// which is what a resident program does, and the tray icon is the only sign it
    /// is running until the hotkey is pressed.
    ///
    /// `stay_open` and `placement` are what this run starts as, `settings` is what the
    /// last run left on disk, and they are two arguments rather than one because a flag
    /// can have put them out of step — see [`crate::settings`].
    ///
    /// Both files arrive already loaded rather than being read here, because which folder
    /// they came out of is `--profile`'s answer and that is the command line's business. The
    /// panel's business is the values.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        stay_open: bool,
        placement: resident::Placement,
        settings: Settings,
        favourites: Favourites,
        resident: resident::Handle,
        start_visible: bool,
    ) -> Self {
        let ctx = &cc.egui_ctx;

        // Azur's `desktop` layer, which is opt-in and names this shape of window in its
        // own first paragraph: "dense, navigational, open all day — a file manager, a
        // process table, a launcher, a mixer". Fonts, the dark palette with the hover
        // rung moved, and the density, in one call.
        //
        // Two of its four rules are why this is a call and not a copy. Rule 1 moves
        // `background-control-hover` *and* `background-card-hover` three rungs up the
        // neutral ramp — one hover grey for a row, a menu entry, a caption button and a
        // hand-painted mute glyph alike — and it moves the **token**, so every fill in
        // this program that reads `bg.control_hover` follows without a call site
        // changing. The bug that module was extracted after is four buttons left behind
        // wearing the old grey because someone moved the rows by hand.
        //
        // Rule 4 is the density: 24px controls, which is what this panel already asked
        // for by hand, and `selectable_labels` off because every mixer row is painted
        // rather than laid out, so a text selection over one is only ever an accident.
        let theme = azur_egui_theme::desktop::install(ctx, true);

        // The outline first, then the handle: the resident thread can show this
        // window the instant it has the handle, and a window shown before DWM has
        // been told what colour to stroke it appears once in the platform's default
        // and then corrects itself, which reads as a flicker on every summon.
        system::adopt_window_outline(cc, theme.stroke.default);
        if let Some(hwnd) = system::window_handle(cc) {
            resident.adopt_panel(hwnd);
        }
        resident.set_placement(placement);

        // Read out before `settings` is moved into the struct, rather than relying on the
        // order the field initialisers happen to be written in.
        let communications_open = settings.communications_open();

        let app = Self {
            theme,
            audio: AudioController::spawn(ctx.clone(), resident.wake()),
            favourites,
            resident,
            stay_open,
            placement,
            settings,
            textures: HashMap::new(),
            mark: None,
            peaks: HashMap::new(),
            echo: Echo::default(),
            output_pick: None,
            input_pick: None,
            communications_output_pick: None,
            communications_input_pick: None,
            focus_seen: false,
            was_focused: false,
            shows_seen: 0,
            communications_open,
        };
        if start_visible {
            // Through the resident thread rather than by starting the window
            // visible, so that `--show` takes exactly the path the hotkey takes:
            // positioned, counted, and with the backend woken.
            app.resident.show();
        } else {
            // One frame beyond eframe's own reveal, so that `App::ui` gets a chance to
            // put the window away. Without this the widget can be left parked and
            // *visible* until something else asks for a repaint, which — with the
            // backend asleep — may be never.
            ctx.request_repaint();
        }
        app
    }
}

impl eframe::App for App {
    /// Transparent, so the panel's own rounded corners are the window's shape.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Runs every frame, *including* the ones where the window is invisible — which is
    /// the only hook that does, and the reason this exists at all.
    ///
    /// One job: keep the widget out of the Alt-Tab switcher. It cannot be done once and
    /// forgotten, because winit re-asserts its own extended style afterwards — see
    /// [`system::keep_out_of_alt_tab`], which reads the style back and writes only when
    /// it has been changed under us.
    fn logic(&mut self, _ctx: &egui::Context, frame: &mut eframe::Frame) {
        system::keep_out_of_alt_tab(frame);
    }

    /// eframe 0.35 hands the app the root `Ui` rather than the `Context`: it spans
    /// the whole window and carries no frame of its own, which is exactly what a
    /// window whose shape is drawn rather than decorated wants.
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // A frame nobody asked for. eframe reveals the window after its first painted
        // frame — see [`crate::PARKED`] — so the first time this is called may be
        // *because* of that reveal rather than because the widget was summoned. The
        // answer is to finish the job: park is where it sits, hidden is what it should
        // be, and the difference matters because a visible window is in the Alt-Tab
        // switcher and a hidden one is not.
        if self.resident.shows() == 0 {
            self.hide();
            return;
        }
        let snapshot = self.audio.snapshot();
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        self.track_peaks(&snapshot, dt);

        // Before the content, so a popup that should be gone does not get a last frame
        // on the way out.
        self.close_stale_popups(&ctx);

        self.hide_on_blur(&ctx);

        // `desktop::panel`: `background-layer`, square, unringed, no margin of its own —
        // rule 3, and the shape this panel had already arrived at by hand. It matters
        // twice over here, because the corner and the outline both belong to the
        // compositor, which rounds this rect and strokes the result in `stroke-default`.
        // Anything painted here would be a second border.
        let frame = azur_egui_theme::desktop::panel(&self.theme);

        // Before the content, so that anything drawn over an edge keeps the
        // pointer: egui gives an overlapping drag to whichever widget was
        // registered last.
        let window = ui.max_rect();
        self.resize_edges(ui, window);

        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            self.chrome(ui, window);
            ui.add(Divider::horizontal());

            egui::Frame::new()
                // `space-4` at the bottom rather than `space-3`: the extra 4px is
                // the strip the resize grip lives in, so the mixer list can never
                // put a row underneath it.
                .inner_margin(egui::Margin {
                    left: space::S3 as i8,
                    right: space::S3 as i8,
                    top: space::S3 as i8,
                    bottom: space::S4 as i8,
                })
                .show(ui, |ui| {
                    // Every gap in this column is an `add_space`, so that the
                    // rhythm can be read off the code. egui would otherwise add
                    // `item_spacing` *as well*, between each pair of items and on
                    // both sides of every space — which is how an 8px gap silently
                    // became 24.
                    ui.spacing_mut().item_spacing.y = 0.0;

                    if let Some(error) = &snapshot.error {
                        ui.add(
                            azur_egui_theme::components::MessageBar::new(error.clone())
                                .intent(azur_egui_theme::components::Intent::Warning),
                        );
                        ui.add_space(space::S3);
                    }

                    self.masters(ui, &snapshot);
                    ui.add_space(space::S3);
                    ui.add(Divider::horizontal());
                    ui.add_space(space::S3);
                    self.devices(ui, &snapshot);
                    ui.add_space(space::S3);
                    ui.add(Divider::horizontal());
                    ui.add_space(space::S3);
                    self.communications(ui, &snapshot);
                    ui.add_space(space::S3);
                    ui.add(Divider::horizontal());
                    ui.add_space(space::S3);
                    self.mixer(ui, &snapshot);
                });
        });

        // Last, so it wins its corner outright.
        self.resize_corner(ui, window);

        // After the content, and consuming rather than reading: a `Select` popup
        // takes Escape to close itself, and if this ran first the panel would be put
        // away by a keystroke the user aimed at a dropdown. Dismissing is hiding now
        // rather than closing — the program outlives the panel, and the way out of it
        // is the tray icon's Quit.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.hide();
        }
    }
}

// ---------------------------------------------------------------------------
// Hiding
// ---------------------------------------------------------------------------

impl App {
    /// Close any popup that belongs to a moment which has passed.
    ///
    /// A menu belongs to a moment. egui has no idea the window went away, so a device list
    /// left open survives a click into another program and is still hanging there when the
    /// panel comes back — over a list that may no longer hold the same devices, which is
    /// the version of this fault that can actually switch the wrong endpoint.
    ///
    /// Two moments close one: the frame the window **loses** the focus, and the frame the
    /// panel is **shown again**. Between them they cover every way a popup can outlive
    /// what it was opened over.
    ///
    /// # Why not `desktop::close_popups_on_blur`
    ///
    /// Because that helper is level-triggered — it closes every popup on every frame while
    /// `!focused` — and this window can be up *without ever having had the focus*. The
    /// resident thread's `SetForegroundWindow` is allowed to fail (see
    /// [`App::hide_on_blur`]), and when it does, a level-triggered close means **no
    /// combobox in this panel can ever open**: the click opens it and the next frame
    /// closes it, for as long as the panel is up. That is the bug this replaced — a panel
    /// that looked perfectly normal, would not drag and would not open a device list,
    /// until it was hidden and summoned again with a keystroke the platform did honour.
    ///
    /// Edge-triggered is also the honest reading of the sentence that helper's name is:
    /// *on* blur, not *while* blurred. And it keeps the promise
    /// [`App::hide_on_blur`] makes — "a panel that never receives the focus at all stays
    /// up until it is dismissed, which is the right way for this to fail" — which is only
    /// true if a panel in that state is still a working panel.
    fn close_stale_popups(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        // Read rather than taken: `hide_on_blur` runs after this and owns `shows_seen`,
        // because what it does with a fresh appearance — forgetting the focus it had — has
        // to happen exactly once too.
        let shown_again = self.resident.shows() != self.shows_seen;
        if popups_are_stale(self.was_focused, focused, shown_again) {
            egui::Popup::close_all(ctx);
        }
        self.was_focused = focused;
    }

    /// Hide the panel when it loses the focus, unless it is set to stay open.
    ///
    /// The whole of what *stay open* means, and the reason it is worth a control: a
    /// panel you summoned to turn one thing down should get out of the way the moment
    /// you look back at what you were doing, and stay only if you asked it to.
    ///
    /// Guarded on having held the focus *since it was last shown*, because `focused`
    /// starts `false` and winit only says otherwise once the window has actually been
    /// given it — without the guard the panel would hide again on the frame it
    /// appeared. That is also why the guard is reset per appearance rather than once:
    /// after the first hide it would otherwise still be set, and the next summon that
    /// did not manage to take the foreground would vanish on its first frame.
    ///
    /// A panel that never receives the focus at all therefore stays up until it is
    /// dismissed, which is the right way for this to fail.
    ///
    /// Only the operating system's focus counts, and that is what makes this safe to
    /// do at all: both `Select` popups and every tooltip in the panel are egui
    /// `Area`s inside this same viewport, so opening one is not a blur — picking a
    /// device cannot dismiss the panel out from under the pointer. Following *Manage
    /// all devices* does dismiss it, because Settings takes the focus, which is what
    /// you want from a row that leaves.
    fn hide_on_blur(&mut self, ctx: &egui::Context) {
        let shows = self.resident.shows();
        if shows != self.shows_seen {
            self.shows_seen = shows;
            self.focus_seen = false;
            // Ask for the focus from *this* side as well, because the resident thread's
            // `SetForegroundWindow` is allowed to fail and does. Windows grants a
            // foreground change to the process that received the last input event, and on
            // the first summon after launch that claim is at its weakest: the hotkey is
            // ours, but a tray click belongs to the shell and `--show` belongs to nobody.
            // A panel that comes up without the focus still *looks* right, which is what
            // made this so hard to see — see [`App::close_stale_popups`] for what it
            // actually cost.
            //
            // This is winit's `focus_window`, from the thread that owns the window, which
            // is the one place the platform is most willing to honour it. It is a request:
            // if it is refused as well, [`App::close_stale_popups`] is what keeps the panel
            // usable anyway.
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }

        if ctx.input(|i| i.focused) {
            self.focus_seen = true;
        } else if self.focus_seen && !self.stay_open {
            self.hide();
        }
    }

    /// Put the panel away. The program stays.
    fn hide(&mut self) {
        // So that the frame this was decided in is not also read as the frame the
        // panel had the focus in, if it is somehow still painting when it comes back.
        self.focus_seen = false;
        self.resident.hide();
    }
}

// ---------------------------------------------------------------------------
// Resizing
// ---------------------------------------------------------------------------

impl App {
    /// Eight invisible strips that resize the window.
    ///
    /// An undecorated winit window on Windows keeps `WS_THICKFRAME` but extends
    /// its client area over the frame, so `WM_NCHITTEST` never reports an edge and
    /// the platform's own resize borders are unreachable. The window is still
    /// resizable — it just has to be asked, which is what
    /// [`ViewportCommand::BeginResize`] does: from there the drag is the platform's
    /// modal resize loop, honouring the aspect-free min size and snapping the same
    /// way any other window does.
    ///
    /// Edges first and corners second, because egui resolves an overlapping drag in
    /// favour of the widget registered last and a corner has to beat the two edges
    /// it sits between.
    fn resize_edges(&self, ui: &Ui, window: Rect) {
        use egui::CursorIcon as Cursor;
        use egui::ResizeDirection as Dir;

        let (l, r, t, b) = (window.left(), window.right(), window.top(), window.bottom());
        let regions: [(&str, Rect, Dir, Cursor); 8] = [
            // Edges, inset by a corner at each end so they never claim one.
            (
                "w",
                Rect::from_min_max(pos2(l, t + CORNER), pos2(l + GRIP, b - CORNER)),
                Dir::West,
                Cursor::ResizeWest,
            ),
            (
                "e",
                Rect::from_min_max(pos2(r - GRIP, t + CORNER), pos2(r, b - CORNER)),
                Dir::East,
                Cursor::ResizeEast,
            ),
            (
                "n",
                Rect::from_min_max(pos2(l + CORNER, t), pos2(r - CORNER, t + GRIP)),
                Dir::North,
                Cursor::ResizeNorth,
            ),
            (
                "s",
                Rect::from_min_max(pos2(l + CORNER, b - GRIP), pos2(r - CORNER, b)),
                Dir::South,
                Cursor::ResizeSouth,
            ),
            // Corners.
            (
                "nw",
                Rect::from_min_max(pos2(l, t), pos2(l + CORNER, t + CORNER)),
                Dir::NorthWest,
                Cursor::ResizeNorthWest,
            ),
            (
                "ne",
                Rect::from_min_max(pos2(r - CORNER, t), pos2(r, t + CORNER)),
                Dir::NorthEast,
                Cursor::ResizeNorthEast,
            ),
            (
                "sw",
                Rect::from_min_max(pos2(l, b - CORNER), pos2(l + CORNER, b)),
                Dir::SouthWest,
                Cursor::ResizeSouthWest,
            ),
            (
                "se",
                Rect::from_min_max(pos2(r - CORNER, b - CORNER), pos2(r, b)),
                Dir::SouthEast,
                Cursor::ResizeSouthEast,
            ),
        ];

        for (name, region, direction, cursor) in regions {
            let response = ui.interact(region, ui.id().with(("resize", name)), Sense::drag());
            if response.hovered() || response.dragged() {
                // After the response, so it beats the `interact_cursor` pointing
                // hand the theme asks for on anything interactive.
                ui.ctx().set_cursor_icon(cursor);
            }
            // `drag_started` rather than the raw press: egui's drag threshold costs
            // a few pixels of travel before the platform loop takes over, which is
            // a fair trade for not starting one on a stray click that lands in the
            // margin.
            if response.drag_started() {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::BeginResize(direction));
            }
        }
    }

    /// The one visible part: a hatch in the bottom-right corner.
    ///
    /// Inside the strip the content column reserves for it — not at the true corner,
    /// which `radius-xlarge` rounds away, and not over the mixer list, which ends
    /// `space-4` above the bottom edge for exactly this.
    fn resize_corner(&self, ui: &Ui, window: Rect) {
        const SIDE: f32 = 10.0;
        widgets::resize_grip(
            ui,
            Rect::from_min_max(
                pos2(
                    window.right() - space::S1 - SIDE,
                    window.bottom() - space::S1 - SIDE,
                ),
                pos2(window.right() - space::S1, window.bottom() - space::S1),
            ),
            &self.theme,
        );
    }
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// Let the window be dragged by `rect`.
///
/// The window is undecorated, so it has no caption for the platform to move it by and
/// every draggable patch of the chrome has to claim itself one. `ui.interact` rather
/// than `allocate_exact_size(.., Sense::click_and_drag())`, because the pixels have
/// already been claimed once — by the mark, by the title, by the space between them —
/// and this is a second, later claim on the same ones. Later wins, and none of those
/// widgets senses a drag to begin with: Azur's `Text` is an `egui::Label`, which senses
/// only hover while `selectable_labels` is off.
///
/// It must stay *after* the buttons for that reason. A single band across the whole row,
/// added last, would take their clicks instead.
///
/// # Why the focus is asked for first
///
/// Measured, on a panel shown without the platform granting it the foreground: `StartDrag`
/// does nothing at all — the window does not move a pixel — where the same drag on the same
/// build moves it 50px once the window is active. winit's `drag_window` is
/// `WM_NCLBUTTONDOWN` with `HTCAPTION`, and the modal move loop that answers it wants an
/// active window; the press that started the drag *would* activate one, but not before the
/// command is sent in this same frame.
///
/// So this asks, one command earlier. It is granted here even when the same request at
/// appearance time was refused, because Windows gives a foreground change to the process
/// that received the last input event and the press that just started this drag is that
/// event. Redundant whenever the panel already has the focus, which is nearly always, and
/// the whole of the difference between a panel you can move and one you cannot when it is
/// not — see [`App::hide_on_blur`] for why that state happens at all.
fn drag_window_from(ui: &Ui, rect: Rect, id: &str) {
    let drag = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

impl App {
    /// Title, drag region, the two toggles, hide.
    ///
    /// Not Azur's `TitleBar`, which draws minimise / maximise / close at 46px each: a
    /// 340px panel has no room for 138px of caption buttons, and a widget that is
    /// resized by its edges has no use for maximise. In their place go the two pieces
    /// of window state a summoned widget actually has, and they are two controls
    /// because they are two questions:
    ///
    /// | toggle | question | see |
    /// | --- | --- | --- |
    /// | stay open | when does it go away? | [`App::hide_on_blur`] |
    /// | where it opens | where does it come back? | [`resident::Placement`] |
    ///
    /// They were one control — a pin — on the theory that a panel you want to keep is
    /// also a panel you have placed. That is true often enough to be tempting and not
    /// often enough to be a design: wanting the panel to survive a click elsewhere says
    /// nothing about whether it should reappear under the pointer.
    fn chrome(&mut self, ui: &mut Ui, window: Rect) {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: space::S3 as i8,
                right: space::S2 as i8,
                top: space::S2 as i8,
                bottom: space::S2 as i8,
            })
            .show(ui, |ui| {
                let row = ui.horizontal(|ui| {
                    // The mark's third surface: the same art as the tray icon and the
                    // `.exe`, from the same bitmaps, at the size this display scale
                    // draws 16 logical pixels as. Its own plate rather than a theme
                    // colour — the art is a render and cannot be recoloured, and on
                    // this header the near-black plate all but disappears, which
                    // leaves the faders reading as the glyph they would have been.
                    let (glyph, _) = ui.allocate_exact_size(Vec2::splat(MARK_BOX), Sense::hover());
                    let mark = self.mark_texture(ui.ctx());
                    ui.painter().image(
                        mark.id(),
                        Rect::from_center_size(glyph.center(), Vec2::splat(MARK)),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    let name = ui.add(Text::new(crate::NAME).variant(TextVariant::BodyStrong));
                    // Handed out to be made draggable below, once the row's height is
                    // known. The mark goes with the name: neither is a control, and a
                    // title bar with a dead patch in it is a title bar people learn to
                    // aim around.
                    let title = glyph.union(name.rect).x_range();

                    let gap = ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                IconButton::new(&azur_icons::close)
                                    .variant(Variant::Subtle)
                                    .size(Size::Small)
                                    // Not "Close": the program keeps running in the
                                    // notification area, and saying "close" of
                                    // something that will still be there afterwards
                                    // is how a tray application loses a user's trust.
                                    .tooltip("Hide (Esc)"),
                            )
                            .clicked()
                        {
                            self.hide();
                        }
                        // Two toggles, and the order is deliberate: *stay open* sits
                        // next to the ✕ because both are about the panel going away.
                        if ui
                            .add(
                                IconButton::new(icons::pin_for(self.stay_open))
                                    .variant(Variant::Subtle)
                                    .size(Size::Small)
                                    .pressed(self.stay_open)
                                    // Said as the consequence rather than as the
                                    // setting: a window that disappears when you click
                                    // away is alarming if you did not know it would.
                                    .tooltip(if self.stay_open {
                                        "Stay open — on. Click to hide it on focus loss"
                                    } else {
                                        "Stay open — off. Click to keep it up and on top"
                                    }),
                            )
                            .clicked()
                        {
                            self.stay_open = !self.stay_open;
                            // A click is the user saying which way they want it, so it
                            // is the one thing that writes the preference down.
                            self.settings.remember_stay_open(self.stay_open);
                            // On top travels with it: a panel that stays open behind
                            // the window you are turning down has not stayed open in
                            // any sense the user meant.
                            ui.ctx().send_viewport_cmd(ViewportCommand::WindowLevel(
                                if self.stay_open {
                                    egui::WindowLevel::AlwaysOnTop
                                } else {
                                    egui::WindowLevel::Normal
                                },
                            ));
                        }
                        if ui
                            .add(
                                IconButton::new(icons::placement_for(self.placement))
                                    .variant(Variant::Subtle)
                                    .size(Size::Small)
                                    // Not `pressed`: neither mode is the "off" one, so
                                    // a pressed state would claim one of them is the
                                    // default and the other an override. The glyph is
                                    // the state.
                                    .tooltip(match self.placement {
                                        resident::Placement::AtPointer => {
                                            "Opens under the mouse. Click to reopen where you left it"
                                        }
                                        resident::Placement::LastPosition => {
                                            "Opens where you left it, on the mouse's screen. Click to follow the mouse"
                                        }
                                    }),
                            )
                            .clicked()
                        {
                            self.placement = match self.placement {
                                resident::Placement::AtPointer => {
                                    resident::Placement::LastPosition
                                }
                                resident::Placement::LastPosition => {
                                    resident::Placement::AtPointer
                                }
                            };
                            // The resident thread is what moves the window, so it is
                            // the one that has to be told; the file, so the next launch
                            // opens where this one was asked to.
                            self.resident.set_placement(self.placement);
                            self.settings.remember_placement(self.placement);
                        }
                        // Leftmost of the four, and the only one that is not about this
                        // window: everything past picking a device — sample rates,
                        // spatial audio, exclusive mode — is the long tail, and the page
                        // that already has it is a better place for it than a 340px
                        // panel. It sits in the chrome rather than in the devices section
                        // because a row that leaves the widget was the one row in that
                        // column not doing what the rows around it do, and because those
                        // 24px belong to the mixer list. The glyph is what says it
                        // leaves; the separation from the three window controls is the
                        // space this layout already puts between the buttons and the
                        // title.
                        if ui
                            .add(
                                IconButton::new(&icons::external)
                                    .variant(Variant::Subtle)
                                    .size(Size::Small)
                                    .tooltip("Windows Audio Settings"),
                            )
                            .clicked()
                        {
                            system::open_sound_settings();
                        }
                        // Whatever is left over between the title and the buttons.
                        // Claimed with `hover` and made draggable below, the same as
                        // the title — `Sense` here would only fight with that.
                        let (gap, _) =
                            ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
                        gap.x_range()
                    });
                    (title, gap.inner)
                });

                // The drag surface, last and therefore on top, and the full height of
                // the row rather than of the widgets inside it — a caption you can only
                // grab in a 24px band through the middle feels broken without ever
                // looking it.
                //
                // Held out of the resize strips, though. The chrome's top margin is
                // `space-2` = 4px and [`GRIP`] is 6, so an unclipped band would cover
                // two pixels of the top edge — and since [`App::resize_edges`] registers
                // *before* the content, those two pixels would drag the window instead
                // of resizing it. Two pixels of a six-pixel edge is exactly the kind of
                // thing that reads as "the top edge is fussy" and never gets diagnosed.
                let (title, gap) = row.inner;
                let row = row.response.rect;
                let height = egui::Rangef::new(
                    row.top().max(window.top() + GRIP),
                    row.bottom().min(window.bottom() - GRIP),
                );
                drag_window_from(ui, Rect::from_x_y_ranges(title, height), "chrome-title");
                drag_window_from(ui, Rect::from_x_y_ranges(gap, height), "chrome-gap");
            });
    }

    /// The two master rows — which are now also the two mute buttons.
    ///
    /// There were a pair of labelled tiles above these saying "Sound on" / "Mic
    /// muted", one click each. They said what the glyph beside each slider already
    /// said, 40px further up and in three times the space, and the glyph is where a
    /// person looks for that anyway — it is what the Windows taskbar and every mixer
    /// row below put the mute on. So the tiles are gone and the glyph carries it:
    /// same shape change, same `danger` colour, and the same affordance the
    /// per-application rows have had all along.
    fn masters(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let output = self.shown("out", snapshot.output);
        let edit = self.volume_row(ui, "master-out", icons::speaker_for(output.muted), output);
        if let Some(value) = edit.volume {
            self.echo.set_volume("out", value);
            self.audio
                .send(Command::SetEndpointVolume(Flow::Output, value));
        }
        if edit.toggle_mute {
            self.set_muted(Flow::Output, !output.muted);
        }

        ui.add_space(space::S2);

        let input = self.shown("in", snapshot.input);
        let edit = self.volume_row(ui, "master-in", icons::microphone_for(input.muted), input);
        if let Some(value) = edit.volume {
            self.echo.set_volume("in", value);
            self.audio
                .send(Command::SetEndpointVolume(Flow::Input, value));
        }
        if edit.toggle_mute {
            self.set_muted(Flow::Input, !input.muted);
        }
    }

    /// An endpoint as its row should draw it: the system's reading, with the local
    /// echo over the top and the smoothed meter level in place of the raw peak.
    fn shown(&mut self, key: &str, reported: Endpoint) -> Endpoint {
        Endpoint {
            volume: self.echo.volume(key, reported.volume),
            muted: self.echo.muted(key, reported.muted),
            peak: self.peaks.get(key).copied().unwrap_or(0.0),
            present: reported.present,
        }
    }

    /// The default device pickers.
    fn devices(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        collection_label(ui, "Default devices");
        ui.add_space(space::S2);

        // `Size::Medium` — 32px on the body step — while everything else in the
        // panel runs at `control-small`. These are the only two controls that carry
        // a whole sentence of text rather than a number or a glyph, and a device
        // name is the one string here worth reading rather than recognising, so they
        // get the size out of step with the rest deliberately.
        self.output_pick = self
            .echo
            .device(Flow::Output, Role::Default, snapshot.default_output);
        let response = ui.add_enabled(
            !snapshot.outputs.is_empty(),
            Select::new(&mut self.output_pick)
                .options(snapshot.outputs.iter().map(|d| d.name.clone()))
                .icon(&icons::speaker)
                .size(Size::Medium)
                .placeholder("No output device"),
        );
        if response.changed() {
            if let Some(device) = self.output_pick.and_then(|i| snapshot.outputs.get(i)) {
                self.echo
                    .set_device(Flow::Output, Role::Default, self.output_pick);
                self.audio
                    .send(Command::SetDefaultDevice(Role::Default, device.id.clone()));
            }
        }

        ui.add_space(space::S2);

        self.input_pick = self
            .echo
            .device(Flow::Input, Role::Default, snapshot.default_input);
        let response = ui.add_enabled(
            !snapshot.inputs.is_empty(),
            Select::new(&mut self.input_pick)
                .options(snapshot.inputs.iter().map(|d| d.name.clone()))
                .icon(&icons::microphone)
                .size(Size::Medium)
                .placeholder("No input device"),
        );
        if response.changed() {
            if let Some(device) = self.input_pick.and_then(|i| snapshot.inputs.get(i)) {
                self.echo
                    .set_device(Flow::Input, Role::Default, self.input_pick);
                self.audio
                    .send(Command::SetDefaultDevice(Role::Default, device.id.clone()));
            }
        }
    }

    /// Where calls go: the same two lists, for Windows' communications role.
    ///
    /// One row wide until it is opened, because this is the setting of somebody who owns a
    /// headset and a pair of speakers and wants each used for what it is good at — a real
    /// want, and not a frequent one. Open, it is the section above again: the same devices,
    /// the same two glyphs, a different role. Which of the two it comes back as is
    /// remembered, so a machine where this matters opens showing it.
    ///
    /// These two are **independent** of the two above them — see
    /// [`Command::SetDefaultDevice`], where each role writes only its own endpoints — and the
    /// heading is the whole of what the panel says about it. A caption said it in words for a
    /// while and is deliberately gone: two lines of `text-tertiary` over the controls they
    /// describe is an explanation the section carries every time it is opened, for a reader
    /// who needed it once, and this panel's answer to "would a sentence help here" has been
    /// no everywhere else. What it explained now lives in the README and in these docs.
    fn communications(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        if widgets::section_toggle(ui, "Communications", self.communications_open).clicked() {
            self.communications_open = !self.communications_open;
            // Down with the two chrome toggles, and for the same reason: a click is the user
            // saying how they want the panel, and being asked twice is being asked too often.
            self.settings
                .remember_communications_open(self.communications_open);
        }
        if !self.communications_open {
            return;
        }

        ui.add_space(space::S2);

        self.communications_output_pick = self.echo.device(
            Flow::Output,
            Role::Communications,
            snapshot.communications_output,
        );
        let response = ui.add_enabled(
            !snapshot.outputs.is_empty(),
            Select::new(&mut self.communications_output_pick)
                .options(snapshot.outputs.iter().map(|d| d.name.clone()))
                .icon(&icons::speaker)
                .size(Size::Medium)
                .placeholder("No output device"),
        );
        if response.changed() {
            if let Some(device) = self
                .communications_output_pick
                .and_then(|i| snapshot.outputs.get(i))
            {
                self.echo.set_device(
                    Flow::Output,
                    Role::Communications,
                    self.communications_output_pick,
                );
                self.audio.send(Command::SetDefaultDevice(
                    Role::Communications,
                    device.id.clone(),
                ));
            }
        }

        ui.add_space(space::S2);

        self.communications_input_pick = self.echo.device(
            Flow::Input,
            Role::Communications,
            snapshot.communications_input,
        );
        let response = ui.add_enabled(
            !snapshot.inputs.is_empty(),
            Select::new(&mut self.communications_input_pick)
                .options(snapshot.inputs.iter().map(|d| d.name.clone()))
                .icon(&icons::microphone)
                .size(Size::Medium)
                .placeholder("No input device"),
        );
        if response.changed() {
            if let Some(device) = self
                .communications_input_pick
                .and_then(|i| snapshot.inputs.get(i))
            {
                self.echo.set_device(
                    Flow::Input,
                    Role::Communications,
                    self.communications_input_pick,
                );
                self.audio.send(Command::SetDefaultDevice(
                    Role::Communications,
                    device.id.clone(),
                ));
            }
        }
    }

    /// The per-application list.
    fn mixer(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        collection_label(ui, "Volume mixer");
        ui.add_space(space::S2);

        if snapshot.sessions.is_empty() {
            ui.add(
                Text::new("Nothing is playing.")
                    .variant(TextVariant::Caption)
                    .color(TextColor::Tertiary),
            );
            return;
        }

        // Three groups, alphabetical inside each: starred, then whatever is actually
        // making sound, then the rest. The two boundaries are the two questions a
        // person opens this panel with — "the one I care about" and "the one that is
        // making noise" — and the pid is the final tiebreak so two instances of the
        // same program never swap places between frames.
        //
        // `!` on both flags is what puts them first: `false` sorts before `true`.
        let mut order: Vec<&Session> = snapshot.sessions.iter().collect();
        order.sort_by(|a, b| {
            let group = |s: &Session| (!self.favourites.contains(&s.name), !s.active);
            group(a)
                .cmp(&group(b))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.pid.cmp(&b.pid))
        });

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Between rows, not after each: a trailing gap under the last one
                // reads as the list having more in it than it does.
                for (i, session) in order.into_iter().enumerate() {
                    if i > 0 {
                        ui.add_space(MIXER_GAP);
                    }
                    self.mixer_row(ui, session);
                }
            });
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

impl App {
    /// A mute button, a percentage, a slider and a meter, on one line.
    ///
    /// The percentage is to the left of the slider rather than in Azur's `Slider`
    /// label row above it: at this width a header line per slider would double the
    /// height of the panel, and the number is the label.
    fn volume_row(
        &self,
        ui: &mut Ui,
        id: &str,
        icon: azur_egui_theme::icons::Icon<'_>,
        state: Endpoint,
    ) -> RowEdit {
        // 20px, the same hit box the per-application mute glyphs get, so the two
        // affordances are literally the same control in two places.
        const GLYPH: f32 = 20.0;
        const NUMBER: f32 = 26.0;
        let t = &self.theme;
        let enabled = state.present;
        let colour = if enabled {
            t.text.secondary
        } else {
            t.text.disabled
        };

        let mut edit = RowEdit::default();
        ui.push_id(id, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S3;

                let (glyph, _) = ui.allocate_exact_size(Vec2::splat(GLYPH), Sense::hover());
                let mute = widgets::glyph_button(
                    ui,
                    glyph,
                    ui.id().with("mute"),
                    icon,
                    if state.muted { t.status.danger } else { colour },
                    if state.muted { "Unmute" } else { "Mute" },
                );
                if mute.clicked() && enabled {
                    edit.toggle_mute = true;
                }

                let (number, _) = ui.allocate_exact_size(vec2(NUMBER, GLYPH), Sense::hover());
                let galley = ui.painter().layout_no_wrap(
                    format!("{:.0}", state.volume * 100.0),
                    t.fonts.caption.clone(),
                    colour,
                );
                // Right-aligned, so 7 / 42 / 100 keep their units column, and on the
                // row's ink baseline so the reading is level with the mute glyph beside
                // it. `caption` is the principal font here because it is the only text
                // on the row.
                let x = number.right() - galley.size().x;
                widgets::galley_on_row(ui, number, &t.fonts.caption, x, galley);

                // The meter's slot is booked before the slider so the handle passes
                // over it; its rect comes from the slider afterwards.
                let slot = widgets::reserve_meter(ui);
                let mut percent = state.volume * 100.0;
                let response =
                    ui.add_enabled(enabled, Slider::new(&mut percent, 0.0..=100.0).step(1.0));
                if response.changed() {
                    edit.volume = Some(percent / 100.0);
                }
                widgets::peak_meter(
                    ui,
                    slot,
                    widgets::meter_rect(response.rect, Size::Medium),
                    state.peak,
                    t,
                );
            });
        });
        edit
    }

    /// One application: identity line, then slider and meter.
    ///
    /// 36px a row — a 20px identity line and a `Size::Small` slider, two apart, with
    /// the meter inside the slider's own row rather than under it. The icon is 20
    /// rather than 24 so it sits inside the line's height instead of setting it,
    /// which is most of the difference between this and a 48px row that shows two
    /// fewer applications.
    ///
    /// So the identity line is *below* `row::DENSE`, which is the densest row Azur
    /// names, and deliberately: `desktop`'s rule 4 is that every point of row height is
    /// a line that did not fit, and 24 here would cost one application per four rows for
    /// a line that already has 20px of glyph in it. What the rule asks for and this
    /// keeps is the other half — the rows touch, with `space-1` between the line and its
    /// own slider and nothing between one application and the next but [`MIXER_GAP`].
    fn mixer_row(&mut self, ui: &mut Ui, session: &Session) {
        const ICON: f32 = 20.0;
        const BUTTON: f32 = 20.0;
        let t = self.theme.clone();

        let muted = self.echo.muted(&session.key, session.muted);
        let volume = self.echo.volume(&session.key, session.volume);
        // Inactive means the session exists but is not rendering — a paused video,
        // a chat client between notifications. Kept in the list, because its volume
        // is still worth setting before the next sound arrives, but receding.
        let dim = !session.active;
        let favourite = self.favourites.contains(&session.name);
        let texture = session
            .icon
            .clone()
            .map(|icon| self.texture(ui.ctx(), &icon));

        ui.push_id(&session.key, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = space::S1;

                // ---- Identity line, painted rather than laid out -------------
                let (line, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ICON), Sense::hover());

                widgets::app_icon(
                    ui,
                    Rect::from_min_size(
                        pos2(line.left(), line.center().y - ICON * 0.5),
                        Vec2::splat(ICON),
                    ),
                    texture.as_ref(),
                    // Nothing to extract an icon from: the system sounds session
                    // has no executable, and a process this one may not open gets
                    // the generic glyph rather than a blank.
                    if session.is_system {
                        &icons::speaker
                    } else {
                        &icons::sliders
                    },
                    t.text.secondary,
                    dim,
                );

                let star_rect = Rect::from_center_size(
                    pos2(line.right() - BUTTON * 0.5, line.center().y),
                    Vec2::splat(BUTTON),
                );
                let mute_rect = Rect::from_center_size(
                    pos2(star_rect.left() - space::S1 - BUTTON * 0.5, line.center().y),
                    Vec2::splat(BUTTON),
                );

                let name_colour = if muted {
                    t.text.disabled
                } else if dim {
                    t.text.tertiary
                } else {
                    t.text.primary
                };
                let name_rect = Rect::from_min_max(
                    pos2(line.left() + ICON + space::S3, line.top()),
                    pos2(mute_rect.left() - space::S3, line.bottom()),
                );
                let galley = widgets::truncated(
                    ui,
                    &session.name,
                    t.fonts.body.clone(),
                    name_colour,
                    name_rect.width(),
                );
                // The line's principal font is `body` -- the name is the only text on
                // it -- and the line holds three glyphs, so the name goes on the ink
                // baseline rather than centred in its box.
                widgets::galley_on_row(ui, line, &t.fonts.body, name_rect.left(), galley);
                // The full name for one that had to be cut, and the pid — which is
                // the only thing that tells two windows of the same browser apart.
                if !session.is_system {
                    ui.interact(name_rect, ui.id().with("name"), Sense::hover())
                        .on_hover_text(format!("{} — process {}", session.name, session.pid));
                }

                if widgets::glyph_button(
                    ui,
                    mute_rect,
                    ui.id().with("mute"),
                    icons::speaker_for(muted),
                    if muted {
                        t.status.danger
                    } else {
                        t.text.secondary
                    },
                    if muted { "Unmute" } else { "Mute" },
                )
                .clicked()
                {
                    self.echo.set_muted(&session.key, !muted);
                    self.audio
                        .send(Command::SetSessionMute(session.key.clone(), !muted));
                }

                if widgets::glyph_button(
                    ui,
                    star_rect,
                    ui.id().with("star"),
                    icons::star_for(favourite),
                    if favourite {
                        // `accent.mark`, not `accent.default`. The dark theme splits the
                        // two on purpose: `default` is the *fill* ramp, two rungs down so
                        // white holds on a primary button, and a filled star is not a
                        // button — it is ink. Measured on this panel, `accent.default` as
                        // ink is 2.75:1 against `bg.layer`, under the 3:1 a shape needs;
                        // `accent.mark` is 6.56:1. It looked fine, which is how it
                        // survived until something measured it.
                        t.accent.mark
                    } else {
                        t.text.tertiary
                    },
                    if favourite {
                        "Unstar — stop keeping it at the top"
                    } else {
                        "Star — keep it at the top"
                    },
                )
                .clicked()
                {
                    self.favourites.toggle(&session.name);
                }

                // ---- Slider and meter ----------------------------------------
                // Slot first, so the handle draws over the meter rather than the
                // other way round.
                let slot = widgets::reserve_meter(ui);
                let mut percent = volume * 100.0;
                let response = ui.add(
                    Slider::new(&mut percent, 0.0..=100.0)
                        .size(Size::Small)
                        .step(1.0),
                );
                if response.changed() {
                    let value = percent / 100.0;
                    self.echo.set_volume(&session.key, value);
                    self.audio
                        .send(Command::SetSessionVolume(session.key.clone(), value));
                }

                let level = if muted {
                    0.0
                } else {
                    self.peaks.get(&session.key).copied().unwrap_or(0.0)
                };
                widgets::peak_meter(
                    ui,
                    slot,
                    widgets::meter_rect(response.rect, Size::Small),
                    level,
                    &t,
                );
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Textures and meters
// ---------------------------------------------------------------------------

impl App {
    /// Upload an application icon once, and hand back the same texture after.
    fn texture(&mut self, ctx: &egui::Context, icon: &Arc<IconImage>) -> TextureHandle {
        let key = Arc::as_ptr(icon) as usize;
        self.textures
            .entry(key)
            .or_insert_with(|| {
                let image = egui::ColorImage::from_rgba_unmultiplied(icon.size, &icon.rgba);
                // Linear, because a 32px icon is drawn at 24 logical pixels and at
                // any display scale in between; nearest would alias every one of
                // those that is not an exact ratio.
                ctx.load_texture(format!("app-icon-{key}"), image, TextureOptions::LINEAR)
            })
            .clone()
    }

    /// This program's own mark, uploaded at the size the header actually draws it.
    ///
    /// Picked off the display scale rather than uploaded once at the largest size:
    /// egui samples a texture bilinearly and has no mipmaps, so a 64px bitmap drawn
    /// into 16 logical pixels is four pixels of every sixteen and the faders come and
    /// go with the window's position. Asking [`crate::mark`] for the physical size
    /// instead gets a bitmap that was scaled to it properly, once, at build time.
    fn mark_texture(&mut self, ctx: &egui::Context) -> TextureHandle {
        let wanted = (MARK * ctx.pixels_per_point()).round().max(1.0) as u32;
        let (size, pixels) = crate::mark::bitmap(wanted);
        match &self.mark {
            Some((uploaded, texture)) if *uploaded == size => texture.clone(),
            _ => {
                let image = egui::ColorImage::from_rgba_unmultiplied([size as usize; 2], pixels);
                // Linear, because 16 logical pixels is 24 physical ones at 150% and
                // the nearest size up is 24 — but at 175% it is 28 physical from a 28,
                // and every scale in between lands off a whole ratio.
                let texture = ctx.load_texture("app-mark", image, TextureOptions::LINEAR);
                self.mark = Some((size, texture.clone()));
                texture
            }
        }
    }

    /// Advance every meter, and forget the ones whose session is gone.
    ///
    /// The sweep is here rather than in a `Drop` somewhere because a session's
    /// disappearance is only ever observed as its absence from a snapshot — there
    /// is no event, and a map that is only ever inserted into is a leak with a slow
    /// fuse in a program that runs for weeks.
    fn track_peaks(&mut self, snapshot: &Snapshot, dt: f32) {
        let mut next: HashMap<String, f32> = HashMap::with_capacity(snapshot.sessions.len() + 2);
        for (key, reading) in [("out", snapshot.output.peak), ("in", snapshot.input.peak)] {
            let previous = self.peaks.get(key).copied().unwrap_or(0.0);
            next.insert(key.to_owned(), widgets::decay_peak(previous, reading, dt));
        }
        for session in &snapshot.sessions {
            let previous = self.peaks.get(&session.key).copied().unwrap_or(0.0);
            next.insert(
                session.key.clone(),
                widgets::decay_peak(previous, session.peak, dt),
            );
        }
        self.peaks = next;

        let live: std::collections::HashSet<usize> = snapshot
            .sessions
            .iter()
            .filter_map(|session| session.icon.as_ref())
            .map(|icon| Arc::as_ptr(icon) as usize)
            .collect();
        self.textures.retain(|key, _| live.contains(key));
    }

    fn set_muted(&mut self, flow: Flow, muted: bool) {
        let key = match flow {
            Flow::Output => "out",
            Flow::Input => "in",
        };
        self.echo.set_muted(key, muted);
        self.audio.send(Command::SetEndpointMute(flow, muted));
    }
}

/// Whether a popup opened before this frame still belongs to what it was opened over.
///
/// Three inputs and one line, pulled out of [`App::close_stale_popups`] because the bug it
/// fixed *was this predicate* — read `focused` where it should have read "focus was just
/// lost", and a panel that never had the focus loses every popup it opens, one frame after
/// opening it. A truth table is cheap; finding that by hand cost an afternoon.
fn popups_are_stale(was_focused: bool, focused: bool, shown_again: bool) -> bool {
    shown_again || (was_focused && !focused)
}

// ---------------------------------------------------------------------------
// Echo
// ---------------------------------------------------------------------------

/// Values the user has just set, held until the backend reads them back.
///
/// Without this, a dragged slider fights the poll: the UI writes 42, the next
/// snapshot still says 38 because it was taken before the write landed, and the
/// handle jumps backwards under the cursor. Holding the local value for [`ECHO`]
/// — or until the reported value agrees, whichever comes first — makes the control
/// feel direct while still deferring to the system as the source of truth.
///
/// The "or until it agrees" half matters as much as the timeout: it is what makes a
/// write that *did* land stop being echoed immediately, so an external change a
/// moment later is not swallowed.
#[derive(Default)]
struct Echo {
    volume: HashMap<String, (f32, Instant)>,
    muted: HashMap<String, (bool, Instant)>,
    /// Keyed by both halves of what a device pick is: which end of the machine, and
    /// which job. The four pickers write four independent values into the same two
    /// lists, so a key of [`Flow`] alone would have the communications section echoing
    /// its answer into the section above it.
    device: HashMap<(Flow, Role), (Option<usize>, Instant)>,
}

impl Echo {
    fn volume(&mut self, key: &str, reported: f32) -> f32 {
        let Some(&(value, until)) = self.volume.get(key) else {
            return reported;
        };
        // 0.5% — under one step of a 0..100 slider, so a value that round-tripped
        // through the audio engine's own quantisation still counts as agreement.
        if Instant::now() >= until || (value - reported).abs() <= 0.005 {
            self.volume.remove(key);
            return reported;
        }
        value
    }

    fn set_volume(&mut self, key: &str, value: f32) {
        self.volume
            .insert(key.to_owned(), (value, Instant::now() + ECHO));
    }

    fn muted(&mut self, key: &str, reported: bool) -> bool {
        let Some(&(value, until)) = self.muted.get(key) else {
            return reported;
        };
        if Instant::now() >= until || value == reported {
            self.muted.remove(key);
            return reported;
        }
        value
    }

    fn set_muted(&mut self, key: &str, value: bool) {
        self.muted
            .insert(key.to_owned(), (value, Instant::now() + ECHO));
    }

    fn device(&mut self, flow: Flow, role: Role, reported: Option<usize>) -> Option<usize> {
        let Some(&(value, until)) = self.device.get(&(flow, role)) else {
            return reported;
        };
        if Instant::now() >= until || value == reported {
            self.device.remove(&(flow, role));
            return reported;
        }
        value
    }

    fn set_device(&mut self, flow: Flow, role: Role, value: Option<usize>) {
        self.device
            .insert((flow, role), (value, Instant::now() + ECHO_DEVICE));
    }
}

#[cfg(test)]
mod tests {
    use super::popups_are_stale;

    /// The predicate behind the bug: a panel that is merely *not focused* keeps its
    /// popups, and only the frame it *loses* the focus takes them away.
    ///
    /// The middle case is the regression. The panel can be up without ever having had the
    /// focus — the resident thread's `SetForegroundWindow` is allowed to fail — and the
    /// version of this that asked "is it focused?" made every combobox in that state
    /// impossible to open: the click opened it and the next frame closed it, for as long as
    /// the panel was up.
    #[test]
    fn only_losing_the_focus_or_being_shown_again_makes_a_popup_stale() {
        assert!(
            popups_are_stale(true, false, false),
            "the blur itself: a click into another program closes the device list"
        );
        assert!(
            !popups_are_stale(false, false, false),
            "never had the focus, and did not just lose it: the popup is the user's"
        );
        assert!(
            !popups_are_stale(true, true, false),
            "still focused, nothing has happened"
        );
        assert!(
            popups_are_stale(false, false, true),
            "a fresh appearance: whatever was open belonged to the last one"
        );
        assert!(
            popups_are_stale(true, true, true),
            "and that holds for an appearance that arrives with the focus"
        );
    }
}
