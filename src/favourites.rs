//! Which applications the user has starred, remembered across restarts.
//!
//! Keyed by display name rather than by session: a session's instance identifier
//! is new on every launch, and the executable path changes under every browser
//! update. The name — "Firefox", "Microsoft Teams" — is the thing the user starred
//! and the thing they expect to still be starred tomorrow.
//!
//! One line per name in a text file under `%APPDATA%`. No format, no dependency,
//! and a file a person can read and edit.

use std::collections::BTreeSet;
use std::path::PathBuf;

pub struct Favourites {
    names: BTreeSet<String>,
    /// `None` when there is no `%APPDATA%` to write to, in which case favourites
    /// still work for this run and are simply not kept.
    file: Option<PathBuf>,
}

impl Favourites {
    pub fn load() -> Self {
        let file = std::env::var_os("APPDATA").map(|appdata| {
            PathBuf::from(appdata)
                .join("Azur Audio Switcher")
                .join("favourites.txt")
        });
        let names = file
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self { names, file }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub fn toggle(&mut self, name: &str) {
        if !self.names.remove(name) {
            self.names.insert(name.to_owned());
        }
        self.save();
    }

    /// Best-effort: a favourite that cannot be written down is still a favourite
    /// for this run, and a widget is not the place to raise a dialog about a
    /// missing directory.
    fn save(&self) {
        let Some(path) = &self.file else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            path,
            self.names.iter().cloned().collect::<Vec<_>>().join("\n"),
        );
    }
}
