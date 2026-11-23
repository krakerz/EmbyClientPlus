//! Keyboard shortcuts in the player, remappable in Preferences → Keyboard.
//! Keys are stored by their GDK names (`space`, `Left`, `k`, `F11`), with
//! Ctrl/Alt/Super as GTK accelerator prefixes (`<Control>f`); only actions
//! changed from their defaults are written to config.

use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::config::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyAction {
    PlayPause,
    SeekBack,
    SeekForward,
    VolumeUp,
    VolumeDown,
    Mute,
    PreviousEpisode,
    NextEpisode,
    PreviousChapter,
    NextChapter,
    Fullscreen,
    Leave,
    Skip,
    Search,
}

impl KeyAction {
    pub const ALL: [KeyAction; 14] = [
        KeyAction::PlayPause,
        KeyAction::SeekBack,
        KeyAction::SeekForward,
        KeyAction::VolumeUp,
        KeyAction::VolumeDown,
        KeyAction::Mute,
        KeyAction::PreviousEpisode,
        KeyAction::NextEpisode,
        KeyAction::PreviousChapter,
        KeyAction::NextChapter,
        KeyAction::Fullscreen,
        KeyAction::Leave,
        KeyAction::Skip,
        KeyAction::Search,
    ];

    /// Config name.
    fn name(self) -> &'static str {
        match self {
            KeyAction::PlayPause => "play_pause",
            KeyAction::SeekBack => "seek_back",
            KeyAction::SeekForward => "seek_forward",
            KeyAction::VolumeUp => "volume_up",
            KeyAction::VolumeDown => "volume_down",
            KeyAction::Mute => "mute",
            KeyAction::PreviousEpisode => "previous_episode",
            KeyAction::NextEpisode => "next_episode",
            KeyAction::PreviousChapter => "previous_chapter",
            KeyAction::NextChapter => "next_chapter",
            KeyAction::Fullscreen => "fullscreen",
            KeyAction::Leave => "leave",
            KeyAction::Skip => "skip",
            KeyAction::Search => "search",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            KeyAction::PlayPause => "Play / pause",
            KeyAction::SeekBack => "Seek back",
            KeyAction::SeekForward => "Seek forward",
            KeyAction::VolumeUp => "Volume up",
            KeyAction::VolumeDown => "Volume down",
            KeyAction::Mute => "Mute",
            KeyAction::PreviousEpisode => "Previous episode",
            KeyAction::NextEpisode => "Next episode",
            KeyAction::PreviousChapter => "Previous chapter",
            KeyAction::NextChapter => "Next chapter",
            KeyAction::Fullscreen => "Fullscreen",
            KeyAction::Leave => "Leave fullscreen, then the player",
            KeyAction::Skip => "Skip intro / credits",
            KeyAction::Search => "Search (outside the player)",
        }
    }

    fn from_name(name: &str) -> Option<KeyAction> {
        KeyAction::ALL.into_iter().find(|a| a.name() == name)
    }

    pub fn default_keys(self) -> &'static [&'static str] {
        match self {
            KeyAction::PlayPause => &["space", "k"],
            KeyAction::SeekBack => &["Left"],
            KeyAction::SeekForward => &["Right"],
            KeyAction::VolumeUp => &["Up"],
            KeyAction::VolumeDown => &["Down"],
            KeyAction::Mute => &["m"],
            KeyAction::PreviousEpisode => &["comma"],
            KeyAction::NextEpisode => &["period"],
            KeyAction::PreviousChapter => &["Page_Up"],
            KeyAction::NextChapter => &["Page_Down"],
            KeyAction::Fullscreen => &["f", "F11"],
            KeyAction::Leave => &["Escape"],
            KeyAction::Skip => &["q"],
            KeyAction::Search => &["<Control>f", "slash"],
        }
    }
}

/// The user's keyboard mapping.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyBindings {
    overrides: BTreeMap<KeyAction, Vec<String>>,
}

impl KeyBindings {
    /// From config; unknown action names are ignored.
    pub fn from_config(config: &BTreeMap<String, Vec<String>>) -> Self {
        let overrides = config
            .iter()
            .filter_map(|(action, keys)| Some((KeyAction::from_name(action)?, keys.clone())))
            .collect();
        KeyBindings { overrides }
    }

    pub fn to_config(&self) -> BTreeMap<String, Vec<String>> {
        self.overrides
            .iter()
            .map(|(action, keys)| (action.name().to_string(), keys.clone()))
            .collect()
    }

    pub fn keys(&self, action: KeyAction) -> Vec<String> {
        self.overrides.get(&action).cloned().unwrap_or_else(|| {
            action
                .default_keys()
                .iter()
                .map(|k| (*k).to_string())
                .collect()
        })
    }

    /// Binds `action` to exactly `key`; any other action using it loses it.
    pub fn set(&mut self, action: KeyAction, key: &str) {
        for other in KeyAction::ALL {
            if other != action && self.keys(other).iter().any(|k| k == key) {
                let remaining = self.keys(other).into_iter().filter(|k| k != key).collect();
                self.store(other, remaining);
            }
        }
        self.store(action, vec![key.to_string()]);
    }

    fn store(&mut self, action: KeyAction, keys: Vec<String>) {
        if keys == action.default_keys() {
            self.overrides.remove(&action);
        } else {
            self.overrides.insert(action, keys);
        }
    }

    /// The action `key` (a GDK key name) triggers, if any.
    pub fn action_for(&self, key: &str) -> Option<KeyAction> {
        KeyAction::ALL
            .into_iter()
            .find(|&action| self.keys(action).iter().any(|k| k == key))
    }
}

thread_local! {
    static BINDINGS: RefCell<Option<KeyBindings>> = const { RefCell::new(None) };
}

/// The mapping in effect (read from config once).
pub fn bindings() -> KeyBindings {
    BINDINGS.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| {
                KeyBindings::from_config(&Settings::load().unwrap_or_default().keyboard.bindings)
            })
            .clone()
    })
}

pub fn set_bindings(bindings: KeyBindings) {
    let config = bindings.to_config();
    if let Err(e) = Settings::update(|s| s.keyboard.bindings = config) {
        tracing::warn!("couldn't save keyboard bindings: {e:#}");
    }
    BINDINGS.with(|cell| cell.replace(Some(bindings)));
}

/// The name a pressed key is stored and matched by: letters lowercase, so
/// Caps Lock or Shift don't change what a key does.
pub fn key_name(key: gtk::gdk::Key) -> Option<String> {
    key.to_lower().name().map(|name| name.to_string())
}

/// `key_name` with the held Ctrl/Alt/Super as `<Control>`-style prefixes.
/// Shift is left out, so `/` stays "slash" on layouts that need it.
pub fn key_press_name(key: gtk::gdk::Key, modifiers: gtk::gdk::ModifierType) -> Option<String> {
    use gtk::gdk::ModifierType as M;
    // On macOS Cmd plays Ctrl's part (Cmd+F), so it's stored as `<Control>` too.
    let primary = modifiers.contains(M::CONTROL_MASK)
        || (cfg!(target_os = "macos") && modifiers.contains(M::META_MASK));
    Some(with_modifiers(
        &key_name(key)?,
        primary,
        modifiers.contains(M::ALT_MASK),
        modifiers.contains(M::SUPER_MASK),
    ))
}

fn with_modifiers(name: &str, control: bool, alt: bool, super_: bool) -> String {
    let mut out = String::new();
    for (held, prefix) in [(control, "<Control>"), (alt, "<Alt>"), (super_, "<Super>")] {
        if held {
            out.push_str(prefix);
        }
    }
    out + name
}

/// Shift, Ctrl, Alt, Super, Caps Lock...: pressed alone they aren't a shortcut.
pub fn is_modifier_key(key: gtk::gdk::Key) -> bool {
    key.name().is_some_and(|name| {
        [
            "Shift",
            "Control",
            "Alt",
            "Super",
            "Meta",
            "Hyper",
            "Caps_Lock",
            "ISO_Level",
        ]
        .iter()
        .any(|m| name.starts_with(m))
    })
}

/// How `<Control>` reads: Cmd stands in for it on macOS.
const PRIMARY_LABEL: &str = if cfg!(target_os = "macos") {
    "Cmd+"
} else {
    "Ctrl+"
};

/// How a stored key name is shown: "Space", "←", "Page Up", "K".
pub fn key_label(name: &str) -> String {
    for (prefix, label) in [
        ("<Control>", PRIMARY_LABEL),
        ("<Alt>", "Alt+"),
        ("<Super>", "Super+"),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return format!("{label}{}", key_label(rest));
        }
    }
    match name {
        "space" => "Space".into(),
        "Left" => "←".into(),
        "Right" => "→".into(),
        "Up" => "↑".into(),
        "Down" => "↓".into(),
        "Page_Up" => "Page Up".into(),
        "Page_Down" => "Page Down".into(),
        "Escape" => "Esc".into(),
        "Return" => "Enter".into(),
        "comma" => ",".into(),
        "period" => ".".into(),
        "slash" => "/".into(),
        other if other.chars().count() == 1 => other.to_uppercase(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_lookup() {
        let bindings = KeyBindings::default();
        assert_eq!(bindings.action_for("space"), Some(KeyAction::PlayPause));
        assert_eq!(bindings.action_for("F11"), Some(KeyAction::Fullscreen));
        assert_eq!(bindings.action_for("q"), Some(KeyAction::Skip));
        assert_eq!(bindings.action_for("period"), Some(KeyAction::NextEpisode));
        assert_eq!(bindings.action_for("z"), None);
    }

    #[test]
    fn remap_moves_a_key_off_its_old_action() {
        let mut bindings = KeyBindings::default();
        // "m" was Mute; now it seeks forward, and Mute has no key left.
        bindings.set(KeyAction::SeekForward, "m");
        assert_eq!(bindings.action_for("m"), Some(KeyAction::SeekForward));
        assert!(bindings.keys(KeyAction::Mute).is_empty());
        assert_eq!(bindings.action_for("Right"), None);
        let round_trip = KeyBindings::from_config(&bindings.to_config());
        assert_eq!(round_trip, bindings);
    }

    #[test]
    fn setting_a_default_back_drops_the_override() {
        let mut bindings = KeyBindings::default();
        bindings.set(KeyAction::Mute, "x");
        bindings.set(KeyAction::Mute, "m");
        assert!(bindings.to_config().is_empty());
    }

    #[test]
    fn labels_read_well() {
        assert_eq!(key_label("space"), "Space");
        assert_eq!(key_label("k"), "K");
        assert_eq!(key_label("Page_Down"), "Page Down");
        assert_eq!(key_label("comma"), ",");
        assert_eq!(key_label("<Control>f"), format!("{PRIMARY_LABEL}F"));
        assert_eq!(
            key_label("<Control><Alt>Left"),
            format!("{PRIMARY_LABEL}Alt+←")
        );
        assert_eq!(key_label("slash"), "/");
    }

    #[test]
    fn search_defaults() {
        let bindings = KeyBindings::default();
        assert_eq!(
            bindings.keys(KeyAction::Search),
            vec!["<Control>f".to_string(), "slash".to_string()]
        );
        assert_eq!(bindings.action_for("<Control>f"), Some(KeyAction::Search));
        assert_eq!(bindings.action_for("slash"), Some(KeyAction::Search));
        // Plain "f" is still Fullscreen.
        assert_eq!(bindings.action_for("f"), Some(KeyAction::Fullscreen));
    }

    #[test]
    fn modifier_names() {
        assert_eq!(with_modifiers("f", false, false, false), "f");
        assert_eq!(with_modifiers("f", true, false, false), "<Control>f");
        assert_eq!(
            with_modifiers("f", true, true, true),
            "<Control><Alt><Super>f"
        );
        assert_eq!(with_modifiers("slash", false, true, false), "<Alt>slash");
    }

    #[test]
    fn old_config_without_modifiers_loads() {
        let config = BTreeMap::from([("mute".to_string(), vec!["x".to_string()])]);
        let bindings = KeyBindings::from_config(&config);
        assert_eq!(bindings.action_for("x"), Some(KeyAction::Mute));
        assert_eq!(bindings.action_for("<Control>f"), Some(KeyAction::Search));
    }
}
