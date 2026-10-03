//! A Windows audio widget: default devices, master volume, and a per-application
//! volume mixer, in the shape the Xbox Game Bar's audio panel put them in.
//!
//! It is resident. Starting it puts an icon in the notification area and nothing on
//! the screen; the panel appears when the hotkey is pressed, and — unless it is set to
//! stay open — puts itself away again the moment you look at something else.
//!
//! ```text
//! azur-audio-switcher                      hidden, Win+Alt+V to open
//! azur-audio-switcher --show               open it straight away
//! azur-audio-switcher --no-stay-open       hides itself when it loses the focus
//! azur-audio-switcher --at-pointer         opens under the mouse, not where you left it
//! azur-audio-switcher --hotkey ctrl+alt+f9 some other keystroke
//! ```
//!
//! Four layers, and the boundaries between them are the point:
//!
//! | Layer | Module | Knows about |
//! | --- | --- | --- |
//! | Backend | [`audio`] | Core Audio, COM, processes. Nothing about egui. |
//! | Resident | [`resident`] | Tray icon, hotkey, and this window's `HWND`. Nothing about the panel's contents. |
//! | Widget | [`app`] | Snapshots and commands. Nothing about COM. |
//! | Glyphs | [`icons`], [`widgets`] | Azur roles, and a `Painter`. |
//!
//! The backend publishes immutable snapshots from its own thread and takes
//! commands over a channel, so the paint loop never blocks on the audio service —
//! see [`audio`] for why that matters more here than the indirection costs. The
//! resident thread exists for a different reason: a hidden window is painted no
//! frames, so the code that has to run while the widget is invisible cannot live in
//! the paint loop at all. See [`resident`].

// A `windows` subsystem binary: without this the widget opens a console behind
// itself. Debug keeps the console, because that is where panics and the backend's
// diagnostics go.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Said out loud rather than left to be discovered as a link error. There is no
// cross-platform volume mixer hiding behind an abstraction here: the backend *is*
// Core Audio, and per-application volume has no equivalent on either of the other
// two desktops that a stub could stand in for.
#[cfg(not(windows))]
compile_error!("Azur Audio Switcher is Windows-only: its backend is Core Audio.");

mod app;
mod audio;
mod favourites;
mod hotkey;
mod icons;
mod mark;
mod resident;
mod system;
mod widgets;

/// What the program is called, in one place.
///
/// Used for the window title, the notification area's tooltip, and the folder the
/// favourites live in — so a rename is a rename rather than a hunt.
pub const NAME: &str = "Azur Audio Switcher";

/// The panel's size at first launch, in logical pixels.
///
/// 340 wide is the Game Bar's own audio widget, and about the narrowest a device
/// name like "Speakers (Realtek(R) Audio)" survives. 620 tall is the ~300px of
/// fixed sections plus seven 40px mixer rows — enough that a typical machine's
/// applications all fit, and small enough not to own a third of the screen.
/// Resize it to taste; the mixer list takes whatever is left over.
const SIZE: [f32; 2] = [340.0, 620.0];

/// Enough for the chrome, both masters, both devices and one mixer row.
///
/// The floor exists so the mixer's scroll area can never be asked for negative
/// height: below this the fixed sections would not fit, and there would be nothing
/// left to scroll in.
const MIN_SIZE: [f32; 2] = [280.0, 360.0];

/// Where the window waits until it is first summoned: far off the desktop.
///
/// It has to wait *somewhere*, because eframe reveals the window itself and there is
/// no asking it not to — `EpiIntegration::post_rendering` calls `set_visible(true)`
/// after the first painted frame to avoid a white flash on startup, whatever the
/// viewport builder said. Hiding it again on the next frame would trade that flash
/// for one of our own, once per launch.
///
/// So the first frame is painted thirty thousand pixels off the corner of the
/// desktop, where being revealed costs nothing, and [`resident`] moves the window
/// into view the first time it is asked for. `with_taskbar(false)` gives the window
/// `WS_EX_TOOLWINDOW`, so it is out of the taskbar and out of Alt-Tab while it sits
/// there.
const PARKED: [f32; 2] = [-30000.0, -30000.0];

/// The size of the window icon handed to the platform at startup.
///
/// 64 rather than 256: Windows scales this one down itself, and at 64 the mark still
/// has every detail it draws while costing 16KB instead of 256.
const WINDOW_ICON: u32 = 64;

/// What the command line asked for.
///
/// The first two are the starting positions of the panel's own two chrome toggles, and
/// they are independent — which is the point of there being two. Neither is remembered
/// between runs, the same as every other piece of window state here.
struct Options {
    /// On top, and stays up until it is dismissed rather than hiding on focus loss.
    stay_open: bool,
    /// Where a summon puts the panel.
    placement: resident::Placement,
    /// `--show`: open the panel at startup instead of waiting to be summoned.
    show: bool,
    hotkey: hotkey::Hotkey,
}

fn main() -> eframe::Result {
    let options = match parse_arguments() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            // Not a panic: a mistyped hotkey is a usage error, and the exit code is
            // the only channel a `windows` subsystem build reliably has.
            std::process::exit(2);
        }
    };

    // Before the window, because the panel's constructor hands its handle straight
    // to this and the thread is what shows it.
    let mut resident = resident::Resident::spawn(options.hotkey.clone(), options.placement);
    let handle = resident.handle();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(NAME)
        .with_inner_size(SIZE)
        .with_min_inner_size(MIN_SIZE)
        // The second of the mark's three surfaces. This window is out of the taskbar
        // and out of Alt-Tab, so nothing shows it today — but egui's default is a
        // white `e` on black, and the cost of not having to remember that if the
        // window ever gains a button is one rasterised bitmap at startup.
        .with_icon(egui::IconData {
            rgba: mark::rgba(WINDOW_ICON),
            width: WINDOW_ICON,
            height: WINDOW_ICON,
        })
        // Undecorated, so the panel is the whole window rather than the inside of
        // someone else's caption bar; transparent, so the compositor can round the
        // corners it draws the outline on and have something to cut away — see
        // `system::adopt_window_outline`.
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(true)
        .with_taskbar(false)
        // Hidden until summoned, whatever `--show` says: showing goes through the
        // resident thread so that every appearance is the same appearance, and that
        // cannot happen before the window exists. `with_visible(false)` holds the
        // reveal until the first frame; `with_position` is what makes the reveal
        // itself invisible. See [`PARKED`].
        .with_visible(false)
        .with_position(PARKED);
    if options.stay_open {
        // On top: a volume panel you have to go looking for behind the window you are
        // trying to turn down is not a widget. Travels with *stay open* rather than
        // being a third setting, because a panel that survives a click elsewhere and
        // then sits behind the window that took the click has not survived it.
        viewport = viewport.with_always_on_top();
    }

    let result = eframe::run_native(
        "Azur Audio Switcher",
        eframe::NativeOptions {
            viewport,
            // The backend, the adapter and the allocator all come from the design system,
            // because all three decide whether a one-pixel stroke reaches the screen as one
            // pixel. See `azur_egui_theme::render` for what was measured.
            //
            // `AZUR_GLOW=1` goes back to the OpenGL painter this used to use. It is lighter,
            // and it is on the path the compositor resamples, so it is an escape hatch for a
            // machine where wgpu will not start rather than a setting worth choosing.
            renderer: if std::env::var_os("AZUR_GLOW").is_some() {
                eframe::Renderer::Glow
            } else {
                eframe::Renderer::Wgpu
            },
            wgpu_options: azur_egui_theme::render::wgpu_options(),
            // A panel of flat rectangles and text: multisampling has nothing to smooth here
            // that egui's one-pixel feathering does not, and dithering — on by default —
            // adds noise to everything sampled from a texture, the glyph atlas included.
            multisampling: 0,
            dithering: false,
            ..Default::default()
        },
        Box::new(move |cc| {
            Ok(Box::new(app::App::new(
                cc,
                options.stay_open,
                options.placement,
                handle,
                options.show,
            )))
        }),
    );

    // The event loop has ended, so the program is going: take the icon out of the
    // notification area rather than leaving a ghost of it behind.
    resident.shutdown();
    result
}

/// Read the command line.
///
/// Hand-rolled rather than a parser crate: four flags, one of which takes a value,
/// and the error messages that matter are [`hotkey::Hotkey::parse`]'s.
fn parse_arguments() -> Result<Options, String> {
    let mut options = Options {
        stay_open: true,
        placement: resident::Placement::LastPosition,
        show: false,
        hotkey: hotkey::Hotkey::parse(hotkey::DEFAULT).expect("the default hotkey parses"),
    };

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--no-stay-open" => options.stay_open = false,
            "--at-pointer" => options.placement = resident::Placement::AtPointer,
            "--last-position" => options.placement = resident::Placement::LastPosition,
            "--show" => options.show = true,
            "--hotkey" => {
                let spec = arguments.next().ok_or_else(|| {
                    "--hotkey needs a keystroke, like --hotkey win+alt+v".to_owned()
                })?;
                options.hotkey = hotkey::Hotkey::parse(&spec)?;
            }
            "--help" | "-h" => {
                // Every line's own indentation is inside the string. A `\` at the end
                // of a line in Rust eats the newline *and* the leading whitespace of
                // the next one, which quietly flattened this to the left margin.
                println!(
                    "{NAME} — a resident audio widget\n\
                     \n\
                     \u{20}   --show            open the panel at startup\n\
                     \u{20}   --no-stay-open    hide it when it loses the focus\n\
                     \u{20}   --at-pointer      open it under the mouse\n\
                     \u{20}   --last-position   open it where you left it (the default),\n\
                     \u{20}                     on whichever screen the mouse is on\n\
                     \u{20}   --hotkey <keys>   what summons it (default {default})\n\
                     \n\
                     The icon in the notification area opens it, and its menu quits.\n\
                     Once open: Esc or the ✕ puts it away, and the two toggles in the\n\
                     header are the same settings as the flags above.",
                    default = hotkey::DEFAULT
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}; try --help")),
        }
    }
    Ok(options)
}
