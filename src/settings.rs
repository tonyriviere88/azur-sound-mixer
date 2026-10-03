//! The panel's two header toggles, remembered across restarts.
//!
//! *Stay open* and *where it opens* are settings rather than window state. They say how
//! the panel should behave, not where it happened to be sitting when it was last put
//! away — and somebody who turns *stay open* off because a panel that sits over their
//! work is the thing they dislike about it does not want to turn it off again tomorrow.
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

use std::path::PathBuf;

use crate::resident::Placement;

/// The panel's behaviour with no file, an unreadable one, or one that does not mention
/// this setting.
///
/// On: a widget that vanishes when you click away is alarming the first time it happens,
/// so the surprising half of the pair is the one you have to ask for.
const DEFAULT_STAY_OPEN: bool = true;

/// The same, for placement. Coming back where you left it is the one that respects a
/// panel somebody has dragged somewhere deliberately.
const DEFAULT_PLACEMENT: Placement = Placement::LastPosition;

/// The remembered value of each toggle, and the file they live in.
pub struct Settings {
    stay_open: bool,
    placement: Placement,
    /// `None` when there is no `%APPDATA%` to write to, in which case the toggles still
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
        let (stay_open, placement) = read(&text);
        Self {
            stay_open,
            placement,
            file,
        }
    }

    /// What the panel should start as, unless the command line says otherwise.
    pub fn stay_open(&self) -> bool {
        self.stay_open
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// Write down that the user turned *stay open* on or off.
    ///
    /// A no-op when the file already says that, so a run whose flag-overridden value
    /// happens to be toggled back into agreement with the file does not rewrite it.
    pub fn remember_stay_open(&mut self, stay_open: bool) {
        if self.stay_open != stay_open {
            self.stay_open = stay_open;
            self.save();
        }
    }

    pub fn remember_placement(&mut self, placement: Placement) {
        if self.placement != placement {
            self.placement = placement;
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
        let _ = std::fs::write(path, write(self.stay_open, self.placement));
    }
}

/// Read a settings file, filling in the defaults for anything it does not say.
///
/// Split from [`Settings::load`] so it can be tested without a `%APPDATA%`.
///
/// Forgiving in three deliberate ways, because the file is meant to be edited by hand:
/// `#` starts a comment, whitespace around either side of the `=` is ignored, and values
/// are matched case-insensitively. A line this version cannot make sense of — an unknown
/// key, a misspelled value, a key with no `=` — leaves its default in place rather than
/// failing the file, so a typo in one line cannot cost the other line its setting.
fn read(text: &str) -> (bool, Placement) {
    let mut stay_open = DEFAULT_STAY_OPEN;
    let mut placement = DEFAULT_PLACEMENT;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (
            key.trim().to_ascii_lowercase().as_str(),
            value.trim().to_ascii_lowercase().as_str(),
        ) {
            ("stay-open", "on") => stay_open = true,
            ("stay-open", "off") => stay_open = false,
            ("placement", "at-pointer") => placement = Placement::AtPointer,
            ("placement", "last-position") => placement = Placement::LastPosition,
            _ => {}
        }
    }
    (stay_open, placement)
}

/// The file [`read`] reads.
///
/// The header names both alternatives for both keys, because the file is the only
/// documentation somebody who opened it in Notepad has: a bare `placement = at-pointer`
/// says nothing about what else could be written there.
fn write(stay_open: bool, placement: Placement) -> String {
    format!(
        "# {name} — the panel's two header toggles.\n\
         # stay-open = on | off\n\
         # placement = at-pointer | last-position\n\
         stay-open = {stay_open}\n\
         placement = {placement}\n",
        name = crate::NAME,
        stay_open = if stay_open { "on" } else { "off" },
        placement = match placement {
            Placement::AtPointer => "at-pointer",
            Placement::LastPosition => "last-position",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{read, write, DEFAULT_PLACEMENT, DEFAULT_STAY_OPEN};
    use crate::resident::Placement;

    /// Every combination survives a write and a read, which is the only property the
    /// panel actually depends on.
    #[test]
    fn what_is_written_is_what_is_read_back() {
        for stay_open in [true, false] {
            for placement in [Placement::AtPointer, Placement::LastPosition] {
                let text = write(stay_open, placement);
                assert_eq!(
                    read(&text),
                    (stay_open, placement),
                    "round trip through:\n{text}"
                );
            }
        }
    }

    /// No file is not an error: it is the first launch, and the first launch has
    /// defaults.
    #[test]
    fn nothing_to_read_means_the_defaults() {
        assert_eq!(read(""), (DEFAULT_STAY_OPEN, DEFAULT_PLACEMENT));
    }

    /// A hand-edited file is the expected case, so the shapes a person produces have to
    /// work: odd spacing, comments, a trailing comment on a live line, and capitals.
    #[test]
    fn a_hand_edited_file_is_read_the_way_it_looks() {
        let text = "# mine\n  Stay-Open   =   OFF   # off for now\nplacement=At-Pointer\n\n";
        assert_eq!(read(text), (false, Placement::AtPointer));
    }

    /// One line it cannot read costs that line's setting and nothing else. Refusing the
    /// whole file would turn a typo into "it forgot everything", which is the failure
    /// mode a hand-editable file must not have.
    #[test]
    fn a_line_it_cannot_read_leaves_the_others_alone() {
        let text = "stay-open = maybe\nverbosity = 11\nplacement\nplacement = at-pointer\n";
        assert_eq!(read(text), (DEFAULT_STAY_OPEN, Placement::AtPointer));
    }

    /// The file explains itself. Asserted because that header is documentation, and
    /// documentation nobody checks is documentation that drifts.
    #[test]
    fn the_file_names_the_values_it_accepts() {
        let text = write(true, Placement::LastPosition);
        for value in ["on", "off", "at-pointer", "last-position"] {
            assert!(
                text.contains(value),
                "the header should name {value:?}:\n{text}"
            );
        }
    }
}
