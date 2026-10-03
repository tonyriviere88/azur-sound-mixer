//! The keystroke that summons the widget, and how it is written down.
//!
//! A global hotkey is taken from every application on the machine, so which one it
//! is cannot be a decision baked into the binary: whatever we picked would be some
//! other program's shortcut for somebody. Hence [`Hotkey::parse`] and `--hotkey`.
//!
//! ```text
//! win+alt+v      ctrl+shift+f9      alt+space
//! ```

use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};

/// `Win+Alt+V` — V for volume.
///
/// Chosen for what it is *not*: `Win`-key combinations are the shell's to give out,
/// so applications do not bind them, which is the property that matters for a
/// shortcut that will be taken away from everything else running. A plain
/// `Ctrl+Alt+`letter would have been quieter to type and would have shadowed a real
/// editor command — `Ctrl+Alt+V` is "introduce variable" in one IDE and paste-special
/// in several other programs.
pub const DEFAULT: &str = "win+alt+v";

/// A parsed hotkey, ready for `RegisterHotKey` and for printing at the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hotkey {
    /// Includes `MOD_NOREPEAT`, so holding the combination down toggles the widget
    /// once rather than sixty times a second.
    pub modifiers: HOT_KEY_MODIFIERS,
    /// A virtual-key code.
    pub key: u16,
    /// How to write it in a tooltip: `Win+Alt+V`.
    pub label: String,
}

impl Hotkey {
    /// Parse `modifier+modifier+key`, case- and space-insensitively.
    ///
    /// At least one modifier is required. A bare key would be registered globally
    /// and would then be swallowed everywhere the user types — refused rather than
    /// honoured, because the failure is silent and total: every `v` in every
    /// document would open a volume panel instead.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut modifiers = HOT_KEY_MODIFIERS(0);
        let mut key: Option<(u16, String)> = None;
        let mut names: Vec<&str> = Vec::new();

        for part in spec.split('+') {
            let part = part.trim();
            if part.is_empty() {
                return Err(format!("{spec:?} has an empty part"));
            }
            match part.to_ascii_lowercase().as_str() {
                "win" | "super" | "meta" => {
                    modifiers |= MOD_WIN;
                    names.push("Win");
                }
                "ctrl" | "control" => {
                    modifiers |= MOD_CONTROL;
                    names.push("Ctrl");
                }
                "alt" => {
                    modifiers |= MOD_ALT;
                    names.push("Alt");
                }
                "shift" => {
                    modifiers |= MOD_SHIFT;
                    names.push("Shift");
                }
                other => {
                    if let Some((_, already)) = &key {
                        return Err(format!(
                            "{spec:?} names two keys, {already:?} and {other:?} — a hotkey has one"
                        ));
                    }
                    key = Some(virtual_key(other).ok_or_else(|| {
                        format!("{other:?} is not a key this understands: a letter, a digit, a function key, or space")
                    })?);
                }
            }
        }

        let Some((key, name)) = key else {
            return Err(format!("{spec:?} names no key, only modifiers"));
        };
        if modifiers == HOT_KEY_MODIFIERS(0) {
            return Err(format!(
                "{spec:?} has no modifier: a bare key registered globally is taken from every program on the machine"
            ));
        }

        // Modifiers in the order Windows writes them, whatever order they arrived in,
        // so two spellings of one hotkey read the same afterwards.
        let mut label = String::new();
        for wanted in ["Win", "Ctrl", "Alt", "Shift"] {
            if names.contains(&wanted) {
                label.push_str(wanted);
                label.push('+');
            }
        }
        label.push_str(&name);

        Ok(Self {
            // Set here rather than asked of the caller: there is no reading of this
            // program in which auto-repeat on a show/hide toggle is what was wanted.
            modifiers: modifiers | MOD_NOREPEAT,
            key,
            label,
        })
    }
}

/// A key name to its virtual-key code, with the label to print for it.
fn virtual_key(name: &str) -> Option<(u16, String)> {
    if name == "space" {
        return Some((0x20, "Space".to_owned()));
    }
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0].to_ascii_uppercase();
        // Letters and digits are their own ASCII codes as virtual keys, which is the
        // one place in the Win32 keyboard API that needs no table.
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some((c as u16, (c as char).to_string()));
        }
        return None;
    }
    if let Some(number) = name.strip_prefix('f').or_else(|| name.strip_prefix('F')) {
        if let Ok(n) = number.parse::<u16>() {
            if (1..=24).contains(&n) {
                // VK_F1 is 0x70 and the rest follow it in order.
                return Some((0x70 + n - 1, format!("F{n}")));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_parses() {
        let hotkey = Hotkey::parse(DEFAULT).expect("default hotkey");
        assert_eq!(hotkey.label, "Win+Alt+V");
        assert_eq!(hotkey.key, b'V' as u16);
        assert!(hotkey.modifiers & MOD_WIN != HOT_KEY_MODIFIERS(0));
        assert!(hotkey.modifiers & MOD_ALT != HOT_KEY_MODIFIERS(0));
    }

    #[test]
    fn no_repeat_is_always_set() {
        assert!(
            Hotkey::parse("ctrl+alt+v").expect("hotkey").modifiers & MOD_NOREPEAT
                != HOT_KEY_MODIFIERS(0)
        );
    }

    #[test]
    fn spelling_and_order_do_not_change_the_label() {
        let a = Hotkey::parse("  ALT + Super + v ").expect("hotkey");
        let b = Hotkey::parse("win+alt+V").expect("hotkey");
        assert_eq!(a.label, b.label);
        assert_eq!(a, b);
    }

    #[test]
    fn function_keys_and_digits_and_space() {
        assert_eq!(Hotkey::parse("ctrl+f9").expect("hotkey").key, 0x78);
        assert_eq!(
            Hotkey::parse("ctrl+shift+f24").expect("hotkey").label,
            "Ctrl+Shift+F24"
        );
        assert_eq!(Hotkey::parse("win+7").expect("hotkey").key, b'7' as u16);
        assert_eq!(
            Hotkey::parse("alt+space").expect("hotkey").label,
            "Alt+Space"
        );
    }

    #[test]
    fn refuses_what_cannot_work() {
        for spec in [
            "v",
            "f9",
            "win",
            "win+alt",
            "win++v",
            "ctrl+f25",
            "ctrl+enter",
            "win+a+b",
        ] {
            assert!(Hotkey::parse(spec).is_err(), "{spec:?} should be refused");
        }
    }
}
