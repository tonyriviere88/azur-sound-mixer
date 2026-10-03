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
//! azur-audio-switcher --profile test       its own settings and favourites
//! ```
//!
//! The panel's two header toggles are remembered between runs, as is whether its
//! *Communications* section is open — see [`settings`] — and a flag that sets one of the
//! toggles (`--no-stay-open`, `--at-pointer`, `--last-position`) overrides what is
//! remembered for that run without replacing it. `--profile` says *which* remembered
//! settings, which is what makes a second copy safe to start — see [`state_directory`].
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

// A `windows` subsystem binary in every profile: no console, ever. This used to be
// `not(debug_assertions)`, on the reasoning that a debug build wants somewhere for panics and
// the backend's diagnostics to go — and the price was a black console window opening behind the
// panel on every debug launch, which is the one thing a resident widget must not do. Anyone
// debugging is running under a debugger or reading a panic through `cargo run`, and
// `system::attach_parent_console` keeps that working: it borrows the console of whatever
// launched this, so `--help` and a usage error still reach a shell without one ever being
// created for a launch from Explorer or the Startup folder.
//
// `not(test)` and not an unconditional attribute, because the test harness is this same crate
// root: a `windows` subsystem test binary has no standard handles, and std reports "no stdio
// handle" as a *successful* write of every byte — so `cargo test` would print nothing at all
// and still exit 0. The one thing worse than a silent program is a silent test run.
#![cfg_attr(not(test), windows_subsystem = "windows")]

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
mod settings;
mod system;
mod widgets;

/// What the program is called, in one place.
///
/// Used for the window title, the notification area's tooltip, and the folder the
/// favourites and the settings live in — so a rename is a rename rather than a hunt.
pub const NAME: &str = "Azur Audio Switcher";

/// Where the two things this program remembers between runs are kept.
///
/// `%APPDATA%\Azur Audio Switcher`, built from [`NAME`] so that the folder follows a
/// rename rather than being spelled out again in [`favourites`] and in [`settings`].
///
/// `profile` is `--profile`, and it is what makes a second copy of the widget safe to run:
/// `Some("test")` moves both files to `%APPDATA%\Azur Audio Switcher\profiles\test`, so
/// starting one to try something out cannot write over the settings and the favourites of the
/// one you actually use. Nested under the default folder rather than beside it, because `None`
/// then resolves to exactly the path it always has — a flag whose whole purpose is to leave
/// your settings alone must not move them in order to introduce itself.
///
/// `None` for `%APPDATA%` itself when the process has none — a service account, or a stripped
/// environment. Both callers treat that as "this run remembers nothing", because a widget that
/// will not start over a missing folder is worse than one that forgets.
fn state_directory(profile: Option<&str>) -> Option<std::path::PathBuf> {
    let directory = std::path::PathBuf::from(std::env::var_os("APPDATA")?).join(NAME);
    Some(match profile {
        Some(profile) => directory.join("profiles").join(profile),
        None => directory,
    })
}

/// Whether `name` is something [`state_directory`] can make a folder out of.
///
/// It goes straight into a path, so this is the check that it cannot walk out of `%APPDATA%`:
/// `--profile ..\..\Startup` is a usage error here rather than a directory somewhere it has no
/// business being. ASCII letters, digits, `-` and `_` — enough for `claude`, `test` or
/// `work-headset`, and nothing that means anything to a filesystem.
///
/// A whitelist rather than a hunt for `..` and separators, because the hunt is the version of
/// this that gets it wrong: `.` alone, a trailing space, a reserved device name like `con`, and
/// an alternate data stream after a colon are all things a path can be that a blacklist has to
/// remember and this cannot express.
fn valid_profile(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

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
/// has every detail it can show while costing 16KB instead of 256. It is also the
/// largest size [`mark`] carries live, so this is an exact hit rather than a round-up.
const WINDOW_ICON: u32 = 64;

/// What the command line asked for.
///
/// The first two are the panel's own two chrome toggles, and they are `Option`s because
/// **not being given is different from being given `false`**: those two are remembered
/// between runs, so what a flag does is override what [`settings`] read for this run only,
/// without replacing it. `None` means "whatever was remembered", and the resolution is two
/// `unwrap_or`s in [`main`] — one place, where both halves are in view. The window's size and
/// position are still not remembered at all.
struct Options {
    /// `--no-stay-open`: on top and up until dismissed, rather than hiding on focus loss.
    stay_open: Option<bool>,
    /// `--at-pointer` / `--last-position`: where a summon puts the panel.
    placement: Option<resident::Placement>,
    /// `--show`: open the panel at startup instead of waiting to be summoned.
    show: bool,
    hotkey: hotkey::Hotkey,
    /// `--profile`: which set of remembered settings this run uses. `None` is the ordinary
    /// one — see [`state_directory`].
    profile: Option<String>,
}

fn main() -> eframe::Result {
    // First, before anything can want to print: this binary has no console of its own in any
    // profile, and this is what lends it the one it was launched from, if it was.
    system::attach_parent_console();

    let options = match parse_arguments() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            // Not a panic: a mistyped hotkey is a usage error, and the exit code is
            // the only channel a `windows` subsystem build reliably has.
            std::process::exit(2);
        }
    };

    warn_about_glow();

    // After the command line, because `--profile` names the folder these come out of, and
    // before the window, because the panel starts as what they say.
    let profile = options.profile.as_deref();
    let settings = settings::Settings::load(profile);
    let favourites = favourites::Favourites::load(profile);

    // The two chrome toggles, resolved: what was remembered, unless this run said otherwise.
    // The whole of the "a flag overrides without replacing" rule is these two lines — the file
    // is not written here, and `App` is what writes it when the user clicks a toggle.
    let stay_open = options.stay_open.unwrap_or(settings.stay_open());
    let placement = options.placement.unwrap_or(settings.placement());

    // Before the window, because the panel's constructor hands its handle straight
    // to this and the thread is what shows it.
    let mut resident = resident::Resident::spawn(options.hotkey.clone(), placement);
    let handle = resident.handle();

    let (side, icon) = mark::bitmap(WINDOW_ICON);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(NAME)
        .with_inner_size(SIZE)
        .with_min_inner_size(MIN_SIZE)
        // The second of the mark's three surfaces. This window is out of the taskbar
        // and out of Alt-Tab, so nothing shows it today — but egui's default is a
        // white `e` on black, and the cost of not having to remember that if the
        // window ever gains a button is one copy of a bitmap at startup.
        .with_icon(egui::IconData {
            rgba: icon.to_vec(),
            width: side,
            height: side,
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
    if stay_open {
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
            // One variant, because `Cargo.toml` compiles one painter: eframe's `glow`
            // feature is gone and `Renderer::Glow` with it. `AZUR_GLOW=1` used to reach
            // the OpenGL painter this program started life on, and what that hatch cost
            // was 313,856 bytes of every copy — `egui_glow`, `glow` and `glutin` — held
            // for a machine where wgpu will not start that nobody has reported. It was
            // also, per `render`'s table, the one path that resamples the hairlines this
            // panel is drawn in: the fallback rendered the design system wrong, which
            // makes it a poor thing to have been paying for. See `warn_about_glow`.
            renderer: eframe::Renderer::Wgpu,
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
                stay_open,
                placement,
                settings,
                favourites,
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

/// Say so if `AZUR_GLOW` is still set somewhere, and carry on.
///
/// The variable used to select the OpenGL painter and there is no longer one to
/// select. Silence would be the wrong answer: someone who set it in a shortcut or a
/// shell profile because wgpu once failed on their machine would get the wgpu window
/// anyway, and conclude the variable is being honoured. This is the same rule
/// [`azur_egui_theme::render`] applies to the `AZUR_BACKEND` names it dropped — a name
/// the program cannot serve is worse than no name at all, so the name goes with the
/// code and what is left says why.
fn warn_about_glow() {
    if let Some(notice) = glow_notice(std::env::var_os("AZUR_GLOW").is_some()) {
        eprintln!("{notice}");
    }
}

/// What [`warn_about_glow`] would say, given whether the variable is set.
///
/// Split out to be testable: the environment is process-global, so a test that set
/// `AZUR_GLOW` would be racing every other test in the binary.
fn glow_notice(set: bool) -> Option<&'static str> {
    set.then_some(
        "AZUR_GLOW is set and no longer does anything: the OpenGL painter is not \
         compiled in. This window renders on wgpu — see azur_egui_theme::render.",
    )
}

/// Read the command line.
///
/// Hand-rolled rather than a parser crate: six flags, two of which take a value, and the
/// error messages that matter are [`hotkey::Hotkey::parse`]'s.
///
/// Reads nothing off disk and takes no defaults from [`settings`], which is why it can run
/// before the settings are loaded — and it has to, because `--profile` is what says *which*
/// settings those are. What it reports is what was asked for and nothing else: the two
/// remembered toggles come back as `None` when their flag was absent, and [`main`] is where
/// that meets the file.
fn parse_arguments() -> Result<Options, String> {
    let mut options = Options {
        stay_open: None,
        placement: None,
        show: false,
        hotkey: hotkey::Hotkey::parse(hotkey::DEFAULT).expect("the default hotkey parses"),
        profile: None,
    };

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--no-stay-open" => options.stay_open = Some(false),
            "--at-pointer" => options.placement = Some(resident::Placement::AtPointer),
            "--last-position" => options.placement = Some(resident::Placement::LastPosition),
            "--show" => options.show = true,
            "--profile" => {
                let name = arguments
                    .next()
                    .ok_or_else(|| "--profile needs a name, like --profile test".to_owned())?;
                if !valid_profile(&name) {
                    // Named rather than sanitised. Quietly turning `../evil` into `evil`
                    // would put the files somewhere the caller did not ask for and could
                    // not predict, and this is a flag whose whole job is being predictable
                    // about where it writes.
                    return Err(format!(
                        "--profile {name:?} cannot be a folder name: letters, digits, \
                         - and _ only, up to 64 of them"
                    ));
                }
                options.profile = Some(name);
            }
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
                     \u{20}   --last-position   open it where you left it, on whichever\n\
                     \u{20}                     screen the mouse is on\n\
                     \u{20}   --hotkey <keys>   what summons it (default {default})\n\
                     \u{20}   --profile <name>  keep this run's settings and favourites\n\
                     \u{20}                     apart from the ones you use\n\
                     \n\
                     The icon in the notification area opens it, and its menu quits.\n\
                     Once open: Esc or the ✕ puts it away, and the two toggles in the\n\
                     header are the same settings as the flags above.\n\
                     \n\
                     Those two toggles are remembered in\n\
                     %APPDATA%\\{name}\\settings.txt, so the three flags that set them\n\
                     are overrides for one run: they change what the panel starts as\n\
                     and not what is written down. Clicking a toggle is what writes.\n\
                     \n\
                     --profile <name> moves that file and favourites.txt into\n\
                     %APPDATA%\\{name}\\profiles\\<name>, which is how you try\n\
                     something out without writing over what you have.",
                    default = hotkey::DEFAULT,
                    name = NAME
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}; try --help")),
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    /// A profile name cannot reach outside `%APPDATA%`.
    ///
    /// The whole of why [`super::valid_profile`] exists: the name is joined onto a path, so
    /// every shape that means something to a filesystem has to be a usage error rather than a
    /// directory. Traversal is the one that matters, and the rest are here because a name that
    /// *looks* accepted and then resolves somewhere else is the same fault wearing a different
    /// hat — `con` and a trailing dot are both legal-looking and neither is a folder Windows
    /// will give you.
    #[test]
    fn a_profile_name_cannot_escape_the_folder_it_names() {
        for name in [
            "", "..", ".", "../evil", "..\\evil", "a/b", "a\\b", "a:b", "a b", "a.", "a*", "é",
        ] {
            assert!(
                !super::valid_profile(name),
                "{name:?} should not be usable as a folder name"
            );
        }
        // 65 characters: one past the cap, which exists so a name cannot be the reason a path
        // runs into `MAX_PATH`.
        assert!(!super::valid_profile(&"a".repeat(65)));

        for name in [
            "claude",
            "test",
            "work-headset",
            "profile_2",
            "A1",
            &"a".repeat(64),
        ] {
            assert!(super::valid_profile(name), "{name:?} should be usable");
        }
    }

    /// A retired override name is diagnosed, not silently ignored.
    ///
    /// The half of this that cannot regress is already enforced by the compiler —
    /// `eframe::Renderer` has one variant now, so nothing can route `AZUR_GLOW` back to
    /// a painter that is not compiled in. What this covers is the other half: that the
    /// variable does not become *silently* inert, which is the failure mode where
    /// somebody keeps it in a shortcut for years believing it does something.
    #[test]
    fn azur_glow_is_reported_as_retired_rather_than_ignored() {
        let notice = super::glow_notice(true).expect("a set AZUR_GLOW is reported");
        assert!(notice.contains("AZUR_GLOW"), "names the variable: {notice}");
        assert!(
            notice.contains("no longer does anything"),
            "says it is inert rather than describing a setting: {notice}"
        );
        assert_eq!(
            super::glow_notice(false),
            None,
            "nothing is printed when the variable is not set"
        );
    }
}
