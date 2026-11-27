//! Validated physical-key bindings shared by preferences, UI and Carbon.
use serde::{Deserialize, Serialize};

pub(crate) const COMMAND: u32 = 1 << 8;
pub(crate) const SHIFT: u32 = 1 << 9;
pub(crate) const OPTION: u32 = 1 << 11;
pub(crate) const CONTROL: u32 = 1 << 12;
const MODIFIERS: u32 = COMMAND | SHIFT | OPTION | CONTROL;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ShortcutMode {
    #[default]
    Toggle,
    Hold,
}

impl ShortcutMode {
    pub(crate) const PREFERENCE: &str = "dictationShortcutMode";

    pub(crate) fn load(saved: Option<&str>) -> Self {
        if saved == Some("hold") {
            Self::Hold
        } else {
            Self::Toggle
        }
    }

    pub(crate) fn preference(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Hold => "hold",
        }
    }

    pub(crate) fn instruction(self) -> &'static str {
        match self {
            Self::Toggle => "开始 / 停止听写",
            Self::Hold => "按住说话，松开结束",
        }
    }

    pub(crate) fn index(self) -> isize {
        isize::from(self == Self::Hold)
    }
}

#[test]
fn shortcut_mode_is_opt_in_and_keeps_unknown_and_old_defaults_in_toggle() {
    assert_eq!(ShortcutMode::default(), ShortcutMode::Toggle);
    for value in [None, Some("toggle"), Some("future"), Some("1")] {
        assert_eq!(ShortcutMode::load(value), ShortcutMode::Toggle);
    }
    assert_eq!(
        ShortcutMode::load(Some(ShortcutMode::Hold.preference())),
        ShortcutMode::Hold
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Shortcut {
    key_code: u16,
    modifiers: u32,
}

impl Default for Shortcut {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Shortcut {
    pub(crate) const DEFAULT: Self = Self {
        key_code: 49,
        modifiers: CONTROL | OPTION,
    };
    pub(crate) const REPASTE: Self = Self {
        key_code: 9,
        modifiers: CONTROL | OPTION | SHIFT,
    };
    pub(crate) const ALTERNATIVE: Self = Self {
        key_code: 49,
        modifiers: CONTROL | SHIFT,
    };

    pub(crate) fn new(key_code: u16, modifiers: u32) -> Result<Self, &'static str> {
        if modifiers & !MODIFIERS != 0 || modifiers & (CONTROL | OPTION | COMMAND) == 0 {
            return Err("请同时按住 Control、Option 或 Command；可再加 Shift。");
        }
        if key_name(key_code).is_none() {
            return Err(
                "请选择字母、数字、符号、空格、方向键或 F1–F20；Esc、Tab 和回车保留用于操作窗口。",
            );
        }
        Ok(Self {
            key_code,
            modifiers,
        })
    }

    pub(crate) fn key_code(self) -> u32 {
        u32::from(self.key_code)
    }
    pub(crate) fn modifiers(self) -> u32 {
        self.modifiers
    }

    /// Convert Carbon registration modifiers to AppKit's event flag positions.
    pub(crate) fn stop_chord(self) -> crate::delivery::model::StopShortcut {
        let mut modifiers = 0;
        for (carbon, appkit) in [
            (SHIFT, 1 << 17),
            (CONTROL, 1 << 18),
            (OPTION, 1 << 19),
            (COMMAND, 1 << 20),
        ] {
            if self.modifiers & carbon != 0 {
                modifiers |= appkit;
            }
        }
        crate::delivery::model::StopShortcut {
            key_code: self.key_code,
            modifiers,
        }
    }

    /// Invalid or stale preference data never reaches native registration.
    /// A missing new preference preserves either legacy preset.
    pub(crate) fn load(saved: Option<&str>, legacy_alternative: bool) -> Self {
        saved
            .and_then(|s| serde_json::from_str::<Self>(s).ok())
            .and_then(|s| Self::new(s.key_code, s.modifiers).ok())
            .unwrap_or(if legacy_alternative {
                Self::ALTERNATIVE
            } else {
                Self::DEFAULT
            })
    }

    pub(crate) fn label(self) -> String {
        let mut label = String::new();
        for (bit, symbol) in [(CONTROL, "⌃"), (OPTION, "⌥"), (SHIFT, "⇧"), (COMMAND, "⌘")] {
            if self.modifiers & bit != 0 {
                label.push_str(symbol);
            }
        }
        label.push_str(key_name(self.key_code).expect("validated shortcut"));
        label
    }

    pub(crate) fn preset_index(self) -> isize {
        if self == Self::DEFAULT {
            0
        } else if self == Self::ALTERNATIVE {
            1
        } else {
            2
        }
    }
}

// Carbon virtual key codes name physical positions, independent of the input
// method. Keep navigation used by the recorder itself out of the accepted set.
fn key_name(code: u16) -> Option<&'static str> {
    Some(match code {
        0 => "A",
        1 => "S",
        2 => "D",
        3 => "F",
        4 => "H",
        5 => "G",
        6 => "Z",
        7 => "X",
        8 => "C",
        9 => "V",
        11 => "B",
        12 => "Q",
        13 => "W",
        14 => "E",
        15 => "R",
        16 => "Y",
        17 => "T",
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        22 => "6",
        23 => "5",
        24 => "=",
        25 => "9",
        26 => "7",
        27 => "-",
        28 => "8",
        29 => "0",
        30 => "]",
        31 => "O",
        32 => "U",
        33 => "[",
        34 => "I",
        35 => "P",
        37 => "L",
        38 => "J",
        39 => "'",
        40 => "K",
        41 => ";",
        42 => "\\",
        43 => ",",
        44 => "/",
        45 => "N",
        46 => "M",
        47 => ".",
        49 => "空格",
        50 => "`",
        64 => "F17",
        79 => "F18",
        80 => "F19",
        90 => "F20",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        111 => "F12",
        113 => "F15",
        115 => "Home",
        116 => "Page Up",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "Page Down",
        122 => "F1",
        123 => "←",
        124 => "→",
        125 => "↓",
        126 => "↑",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_both_legacy_presets_and_prefers_valid_custom_data() {
        assert_eq!(Shortcut::load(None, false), Shortcut::DEFAULT);
        assert_eq!(Shortcut::load(None, true), Shortcut::ALTERNATIVE);
        let custom = Shortcut::new(40, CONTROL | OPTION | SHIFT).unwrap();
        let saved = serde_json::to_string(&custom).unwrap();
        assert_eq!(Shortcut::load(Some(&saved), true), custom);
        assert_eq!(custom.label(), "⌃⌥⇧K");
        assert_eq!(Shortcut::load(Some(&saved), false).preset_index(), 2);
    }

    #[test]
    fn rejects_corrupt_unsafe_or_unknown_preferences() {
        for saved in [
            "",
            "null",
            "{}",
            "false",
            r#"{"key_code":40,"modifiers":0}"#,
            r#"{"key_code":53,"modifiers":4096}"#,
            r#"{"key_code":400,"modifiers":4096}"#,
            r#"{"key_code":40,"modifiers":4294967295}"#,
            r#"{"key_code":40,"modifiers":4096,"extra":true}"#,
        ] {
            assert_eq!(
                Shortcut::load(Some(saved), false),
                Shortcut::DEFAULT,
                "{saved}"
            );
            assert_eq!(
                Shortcut::load(Some(saved), true),
                Shortcut::ALTERNATIVE,
                "{saved}"
            );
        }
    }

    #[test]
    fn converts_every_registered_modifier_combination_for_input_monitoring() {
        use crate::delivery::model::invalidates_input;
        for bits in 0..16 {
            let mut carbon = 0;
            let mut appkit = 0;
            for (index, native, flag) in [
                (0, SHIFT, 1 << 17),
                (1, CONTROL, 1 << 18),
                (2, OPTION, 1 << 19),
                (3, COMMAND, 1 << 20),
            ] {
                if bits & (1 << index) != 0 {
                    carbon |= native;
                    appkit |= flag;
                }
            }
            let Ok(shortcut) = Shortcut::new(0, carbon) else {
                continue;
            };
            let stop = shortcut.stop_chord();
            assert_eq!(stop.key_code, 0);
            assert_eq!(stop.modifiers, appkit);
            assert!(!invalidates_input(0, true, 0, appkit, Some(stop)));
            assert!(invalidates_input(0, true, 49, appkit, Some(stop)));
        }
    }

    #[test]
    fn validates_real_combinations_and_keeps_dialog_navigation_available() {
        for modifiers in [0, SHIFT, 1 << 20, CONTROL | (1 << 20)] {
            assert!(Shortcut::new(40, modifiers).is_err());
        }
        for code in [36, 48, 51, 53, 55, 56, 57, 59, 63, 76, 255] {
            assert!(Shortcut::new(code, CONTROL).is_err());
        }
        for code in [0, 18, 42, 49, 64, 122, 126] {
            for modifiers in [CONTROL, OPTION, COMMAND, CONTROL | OPTION | SHIFT | COMMAND] {
                let s = Shortcut::new(code, modifiers).unwrap();
                assert!(!s.label().is_empty());
                assert_eq!(
                    Shortcut::load(Some(&serde_json::to_string(&s).unwrap()), false),
                    s
                );
            }
        }
        assert_eq!(Shortcut::DEFAULT.label(), "⌃⌥空格");
        assert_eq!(Shortcut::ALTERNATIVE.label(), "⌃⇧空格");
    }
}
