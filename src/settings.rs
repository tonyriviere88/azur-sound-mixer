//! What the panel remembers between runs: two header toggles and one open section.
//!
//! *Stay open*, *where it opens* and whether the *Communications* section is showing its
//! pickers are settings rather than window state. They say how the panel should behave and
//! what of it there is to look at, not where it happened to be sitting when it was last put
//! away — and somebody who turns *stay open* off because a panel that sits over their work
//! is the thing they dislike about it does not want to turn it off again tomorrow.
//!
//! One `key = value` line each, in a text file beside the favourites, for the same
//! reasons [`crate::favourites`] gives: no format, no dependency, and a file a person
//! can read and edit. The values are spelled the way the command line spells them —
//! `on`/`off`, `at-pointer`/`last-position` — so the file uses names `--help` has
//! already shown.
//!
//! # This is what is remembered, not what the panel is doing
//!
//! A command-line flag overrides a remembered setting **for that run, and does not
//! replace it**. `--no-stay-open` typed into a shell is a decision about this launch;
//! answering it by rewriting the preference would quietly make a one-off permanent, and
//! the flag would then be no use as a try-it-once.
//!
//! So there are two copies of each value on purpose: this struct holds what is on disk,
//! [`crate::app::App`] holds what is in force, and they differ for exactly as long as a
//! flag is overriding one. Clicking the toggle is what brings them back together,
//! because clicking the toggle is the only thing here that writes.
//!
//! The section is the one of the three with no flag, because there is nothing a launch could
//! want to say about it that opening it would not say better.

use std::path::PathBuf;

use crate::resident::Placement;

/// What the panel is with no file, an unreadable one, or one that does not mention a key.
///
/// *Stay open* on: a widget that vanishes when you click away is alarming the first time it
/// happens, so the surprising half of that pair is the one you have to ask for. *Where it
/// opens* at the last position, which is the one that respects a panel somebody has dragged
/// somewhere deliberately. The section shut, because it holds the setting of somebody who owns
/// a headset *and* speakers, and the panel should not open 80px taller for everybody else.
const DEFAULTS: Values = Values {
    stay_open: true,
    placement: Placement::LastPosition,
    communications_open: false,
};

/// The three values a settings file carries.
///
/// A struct rather than the tuple this was when there were two of them, and that is not
/// tidiness: two of the three are booleans, so a tuple lets a caller — or a test — swap them
/// and still compile. Naming them is what makes that impossible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Values {
    stay_open: bool,
    placement: Placement,
    communications_open: bool,
}

/// What is remembered, and the file it lives in.
pub struct Settings {
    values: Values,
    /// `None` when there is no `%APPDATA%` to write to, in which case all three still
    /// work for this run and are simply not kept.
    file: Option<PathBuf>,
}

impl Settings {
    /// `profile` is `--profile`, and it decides which folder is read — see
    /// [`crate::state_directory`]. Passed in rather than read from the environment here,
    /// because the flag that names it is the command line's and this module has no business
    /// parsing one.
    pub fn load(profile: Option<&str>) -> Self {
        let file = crate::state_directory(profile).map(|directory| directory.join("settings.txt"));
        let text = file
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        Self {
            values: read(&text),
            file,
        }
    }

    /// What the panel should start as, unless the command line says otherwise.
    pub fn stay_open(&self) -> bool {
        self.values.stay_open
    }

    pub fn placement(&self) -> Placement {
        self.values.placement
    }

    /// Whether the *Communications* section starts open. No flag overrides this one, so it
    /// is what the panel does.
    pub fn communications_open(&self) -> bool {
        self.values.communications_open
    }

    /// Write down that the user turned *stay open* on or off.
    ///
    /// A no-op when the file already says that, so a run whose flag-overridden value
    /// happens to be toggled back into agreement with the file does not rewrite it.
    pub fn remember_stay_open(&mut self, stay_open: bool) {
        self.remember(|values| values.stay_open = stay_open);
    }

    pub fn remember_placement(&mut self, placement: Placement) {
        self.remember(|values| values.placement = placement);
    }

    pub fn remember_communications_open(&mut self, open: bool) {
        self.remember(|values| values.communications_open = open);
    }

    /// Change one value and write the file — unless nothing changed.
    ///
    /// The guard is the whole reason the three setters go through one function: every one of
    /// them is called from a click handler, and two of the three can be clicked into agreeing
    /// with what is already on disk (a flag-overridden run toggled back). Comparing the whole
    /// of [`Values`] rather than the field is what makes that check impossible to forget when
    /// a fourth setting arrives.
    fn remember(&mut self, change: impl FnOnce(&mut Values)) {
        let mut values = self.values;
        change(&mut values);
        if values != self.values {
            self.values = values;
            self.save();
        }
    }

    /// Best-effort, the same as [`crate::favourites`]: a setting that cannot be written
    /// down is still in force for this run, and a toggle in a header is not the place to
    /// raise a dialog about a missing directory.
    fn save(&self) {
        let Some(path) = &self.file else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, write(&self.values));
    }
}

/// Read a settings file, filling in [`DEFAULTS`] for anything it does not say.
///
/// Split from [`Settings::load`] so it can be tested without a `%APPDATA%`.
///
/// Forgiving in three deliberate ways, because the file is meant to be edited by hand:
/// `#` starts a comment, whitespace around either side of the `=` is ignored, and values
/// are matched case-insensitively. A line this version cannot make sense of — an unknown
/// key, a misspelled value, a key with no `=` — leaves its default in place rather than
/// failing the file, so a typo in one line cannot cost the other lines their settings.
fn read(text: &str) -> Values {
    let mut values = DEFAULTS;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (
            key.trim().to_ascii_lowercase().as_str(),
            value.trim().to_ascii_lowercase().as_str(),
        ) {
            ("stay-open", "on") => values.stay_open = true,
            ("stay-open", "off") => values.stay_open = false,
            ("placement", "at-pointer") => values.placement = Placement::AtPointer,
            ("placement", "last-position") => values.placement = Placement::LastPosition,
            ("communications", "open") => values.communications_open = true,
            ("communications", "shut") => values.communications_open = false,
            _ => {}
        }
    }
    values
}

/// The file [`read`] reads.
///
/// The header names every alternative for every key, because the file is the only
/// documentation somebody who opened it in Notepad has: a bare `placement = at-pointer`
/// says nothing about what else could be written there.
///
/// `open`/`shut` for the section rather than another `on`/`off`, because a line reading
/// `communications = off` is one somebody would take for "do not use a communications device"
/// — which is not a setting this program has, and would be an alarming one to think you had
/// turned on by accident.
fn write(values: &Values) -> String {
    format!(
        "# {name} — what the panel remembers.\n\
         # stay-open      = on | off\n\
         # placement      = at-pointer | last-position\n\
         # communications = open | shut\n\
         stay-open = {stay_open}\n\
         placement = {placement}\n\
         communications = {communications}\n",
        name = crate::NAME,
        stay_open = if values.stay_open { "on" } else { "off" },
        placement = match values.placement {
            Placement::AtPointer => "at-pointer",
            Placement::LastPosition => "last-position",
        },
        communications = if values.communications_open {
            "open"
        } else {
            "shut"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{read, write, Values, DEFAULTS};
    use crate::resident::Placement;

    /// Every combination survives a write and a read, which is the only property the
    /// panel actually depends on.
    ///
    /// All eight rather than a sample, because the cost of the whole cross product is nothing
    /// and what it catches is the copy-paste fault this shape of code has: two booleans
    /// written from the same `if`, or read into the same field.
    #[test]
    fn what_is_written_is_what_is_read_back() {
        for stay_open in [true, false] {
            for placement in [Placement::AtPointer, Placement::LastPosition] {
                for communications_open in [true, false] {
                    let values = Values {
                        stay_open,
                        placement,
                        communications_open,
                    };
                    let text = write(&values);
                    assert_eq!(read(&text), values, "round trip through:\n{text}");
                }
            }
        }
    }

    /// No file is not an error: it is the first launch, and the first launch has defaults —
    /// including a *Communications* section that is shut.
    #[test]
    fn nothing_to_read_means_the_defaults() {
        assert_eq!(read(""), DEFAULTS);
        // Through `read` rather than off `DEFAULTS`, which would be an assertion about a
        // constant: the claim being pinned is the behaviour — a machine with no settings file
        // opens the panel with the section shut.
        assert!(
            !read("").communications_open,
            "the section is shut until somebody opens it"
        );
    }

    /// A hand-edited file is the expected case, so the shapes a person produces have to
    /// work: odd spacing, comments, a trailing comment on a live line, and capitals.
    #[test]
    fn a_hand_edited_file_is_read_the_way_it_looks() {
        let text = "# mine\n  Stay-Open   =   OFF   # off for now\nplacement=At-Pointer\n\
                    COMMUNICATIONS = Open\n\n";
        assert_eq!(
            read(text),
            Values {
                stay_open: false,
                placement: Placement::AtPointer,
                communications_open: true,
            }
        );
    }

    /// One line it cannot read costs that line's setting and nothing else. Refusing the
    /// whole file would turn a typo into "it forgot everything", which is the failure
    /// mode a hand-editable file must not have.
    #[test]
    fn a_line_it_cannot_read_leaves_the_others_alone() {
        let text = "stay-open = maybe\nverbosity = 11\nplacement\ncommunications = ajar\n\
                    placement = at-pointer\n";
        assert_eq!(
            read(text),
            Values {
                placement: Placement::AtPointer,
                ..DEFAULTS
            }
        );
    }

    /// The file explains itself. Asserted because that header is documentation, and
    /// documentation nobody checks is documentation that drifts.
    #[test]
    fn the_file_names_the_values_it_accepts() {
        let text = write(&DEFAULTS);
        for value in ["on", "off", "at-pointer", "last-position", "open", "shut"] {
            assert!(
                text.contains(value),
                "the header should name {value:?}:\n{text}"
            );
        }
    }
}
