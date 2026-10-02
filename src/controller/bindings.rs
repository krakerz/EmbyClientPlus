//! Controller buttons, the actions they trigger, and the user's mapping
//! between them. Pure data, independent of gilrs and GTK.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A controller input, named the Xbox way (what Steam Input presents).
/// Stick directions arrive as the D-pad ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pad {
    A,
    B,
    X,
    Y,
    LB,
    RB,
    LT,
    RT,
    Select,
    Start,
    L3,
    R3,
    Up,
    Down,
    Left,
    Right,
}

impl Pad {
    pub const ALL: [Pad; 16] = [
        Pad::A,
        Pad::B,
        Pad::X,
        Pad::Y,
        Pad::LB,
        Pad::RB,
        Pad::LT,
        Pad::RT,
        Pad::Select,
        Pad::Start,
        Pad::L3,
        Pad::R3,
        Pad::Up,
        Pad::Down,
        Pad::Left,
        Pad::Right,
    ];

    /// Config name, e.g. "LB".
    pub fn name(self) -> &'static str {
        match self {
            Pad::A => "A",
            Pad::B => "B",
            Pad::X => "X",
            Pad::Y => "Y",
            Pad::LB => "LB",
            Pad::RB => "RB",
            Pad::LT => "LT",
            Pad::RT => "RT",
            Pad::Select => "Select",
            Pad::Start => "Start",
            Pad::L3 => "L3",
            Pad::R3 => "R3",
            Pad::Up => "Up",
            Pad::Down => "Down",
            Pad::Left => "Left",
            Pad::Right => "Right",
        }
    }

    pub fn from_name(name: &str) -> Option<Pad> {
        Pad::ALL
            .into_iter()
            .find(|pad| pad.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Label for the Preferences rows.
    pub fn label(self) -> &'static str {
        match self {
            Pad::Up => "D-pad ↑",
            Pad::Down => "D-pad ↓",
            Pad::Left => "D-pad ←",
            Pad::Right => "D-pad →",
            Pad::Select => "Select / View",
            Pad::Start => "Start / Menu",
            other => other.name(),
        }
    }

    pub fn is_direction(self) -> bool {
        matches!(self, Pad::Up | Pad::Down | Pad::Left | Pad::Right)
    }
}

/// Where an action applies: browsing pages, or the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Browse,
    Player,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    // Browsing.
    Up,
    Down,
    Left,
    Right,
    Activate,
    Back,
    Home,
    ContextMenu,
    PreviousTab,
    NextTab,
    Search,
    Preferences,
    // Player.
    PlayPause,
    Leave,
    SeekBack,
    SeekForward,
    VolumeUp,
    VolumeDown,
    PreviousEpisode,
    NextEpisode,
    PreviousChapter,
    NextChapter,
    AudioMenu,
    SubtitleMenu,
    ShowControls,
    Skip,
}

impl Action {
    pub const ALL: [Action; 26] = [
        Action::Up,
        Action::Down,
        Action::Left,
        Action::Right,
        Action::Activate,
        Action::Back,
        Action::Home,
        Action::ContextMenu,
        Action::PreviousTab,
        Action::NextTab,
        Action::Search,
        Action::Preferences,
        Action::PlayPause,
        Action::Leave,
        Action::SeekBack,
        Action::SeekForward,
        Action::VolumeUp,
        Action::VolumeDown,
        Action::PreviousEpisode,
        Action::NextEpisode,
        Action::PreviousChapter,
        Action::NextChapter,
        Action::AudioMenu,
        Action::SubtitleMenu,
        Action::ShowControls,
        Action::Skip,
    ];

    pub fn context(self) -> Context {
        if self >= Action::PlayPause {
            Context::Player
        } else {
            Context::Browse
        }
    }

    /// Config key.
    pub fn name(self) -> &'static str {
        match self {
            Action::Up => "up",
            Action::Down => "down",
            Action::Left => "left",
            Action::Right => "right",
            Action::Activate => "activate",
            Action::Back => "back",
            Action::Home => "home",
            Action::ContextMenu => "context_menu",
            Action::PreviousTab => "previous_tab",
            Action::NextTab => "next_tab",
            Action::Search => "search",
            Action::Preferences => "preferences",
            Action::PlayPause => "play_pause",
            Action::Leave => "leave",
            Action::SeekBack => "seek_back",
            Action::SeekForward => "seek_forward",
            Action::VolumeUp => "volume_up",
            Action::VolumeDown => "volume_down",
            Action::PreviousEpisode => "previous_episode",
            Action::NextEpisode => "next_episode",
            Action::PreviousChapter => "previous_chapter",
            Action::NextChapter => "next_chapter",
            Action::AudioMenu => "audio_menu",
            Action::SubtitleMenu => "subtitle_menu",
            Action::ShowControls => "show_controls",
            Action::Skip => "skip",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Up => "Move up",
            Action::Down => "Move down",
            Action::Left => "Move left",
            Action::Right => "Move right",
            Action::Activate => "Select",
            Action::Back => "Back",
            Action::Home => "Home",
            Action::ContextMenu => "Item menu",
            Action::PreviousTab => "Previous tab / season",
            Action::NextTab => "Next tab / season",
            Action::Search => "Search",
            Action::Preferences => "Preferences",
            Action::PlayPause => "Play / pause",
            Action::Leave => "Leave player",
            Action::SeekBack => "Seek back 10 s",
            Action::SeekForward => "Seek forward 10 s",
            Action::VolumeUp => "Volume up",
            Action::VolumeDown => "Volume down",
            Action::PreviousEpisode => "Previous episode",
            Action::NextEpisode => "Next episode",
            Action::PreviousChapter => "Previous chapter",
            Action::NextChapter => "Next chapter",
            Action::AudioMenu => "Audio menu",
            Action::SubtitleMenu => "Subtitle menu",
            Action::ShowControls => "Show controls",
            Action::Skip => "Skip intro/credits",
        }
    }

    fn from_name(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.name() == name)
    }

    pub fn default_buttons(self) -> &'static [Pad] {
        match self {
            Action::Up | Action::VolumeUp => &[Pad::Up],
            Action::Down | Action::VolumeDown => &[Pad::Down],
            Action::Left | Action::SeekBack => &[Pad::Left],
            Action::Right | Action::SeekForward => &[Pad::Right],
            Action::Activate | Action::PlayPause => &[Pad::A],
            Action::Back | Action::Leave => &[Pad::B],
            Action::Home => &[Pad::Y],
            Action::ContextMenu | Action::AudioMenu => &[Pad::X],
            Action::SubtitleMenu => &[Pad::Y],
            Action::PreviousTab | Action::PreviousEpisode => &[Pad::LB],
            Action::NextTab | Action::NextEpisode => &[Pad::RB],
            Action::PreviousChapter => &[Pad::LT],
            Action::NextChapter => &[Pad::RT],
            Action::Search | Action::Skip => &[Pad::Select],
            Action::Preferences | Action::ShowControls => &[Pad::Start],
        }
    }
}

/// The user's mapping. Stored sparsely: only actions changed from their
/// defaults are written to config.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bindings {
    overrides: BTreeMap<Action, Vec<Pad>>,
}

impl Bindings {
    /// From config (`action name → button names`); unknown names are
    /// ignored so old or hand-edited configs never break input.
    pub fn from_config(config: &BTreeMap<String, Vec<String>>) -> Self {
        let overrides = config
            .iter()
            .filter_map(|(action, buttons)| {
                let action = Action::from_name(action)?;
                let buttons: Vec<Pad> = buttons.iter().filter_map(|b| Pad::from_name(b)).collect();
                Some((action, buttons))
            })
            .collect();
        Bindings { overrides }
    }

    pub fn to_config(&self) -> BTreeMap<String, Vec<String>> {
        self.overrides
            .iter()
            .map(|(action, buttons)| {
                (
                    action.name().to_string(),
                    buttons.iter().map(|b| b.name().to_string()).collect(),
                )
            })
            .collect()
    }

    pub fn buttons(&self, action: Action) -> Vec<Pad> {
        self.overrides
            .get(&action)
            .cloned()
            .unwrap_or_else(|| action.default_buttons().to_vec())
    }

    /// Binds `action` to exactly `pad`. Other actions in the same context
    /// using `pad` lose it, so one press never means two things.
    pub fn set(&mut self, action: Action, pad: Pad) {
        for other in Action::ALL {
            if other != action
                && other.context() == action.context()
                && self.buttons(other).contains(&pad)
            {
                let remaining: Vec<Pad> = self
                    .buttons(other)
                    .into_iter()
                    .filter(|&b| b != pad)
                    .collect();
                self.store(other, remaining);
            }
        }
        self.store(action, vec![pad]);
    }

    fn store(&mut self, action: Action, buttons: Vec<Pad>) {
        if buttons == action.default_buttons() {
            self.overrides.remove(&action);
        } else {
            self.overrides.insert(action, buttons);
        }
    }

    /// The action `pad` triggers in `context`, if any.
    pub fn action_for(&self, pad: Pad, context: Context) -> Option<Action> {
        Action::ALL
            .into_iter()
            .filter(|action| action.context() == context)
            .find(|&action| self.buttons(action).contains(&pad))
    }
}

/// Hold-to-repeat for directions: one press, then after `DELAY` a repeat
/// every `INTERVAL` while held.
#[derive(Debug, Default)]
pub struct Repeater {
    held: Vec<(Pad, Instant)>,
}

impl Repeater {
    pub const DELAY: Duration = Duration::from_millis(400);
    pub const INTERVAL: Duration = Duration::from_millis(120);

    pub fn press(&mut self, pad: Pad, now: Instant) {
        if pad.is_direction() && !self.held.iter().any(|(held, _)| *held == pad) {
            self.held.push((pad, now + Self::DELAY));
        }
    }

    pub fn release(&mut self, pad: Pad) {
        self.held.retain(|(held, _)| *held != pad);
    }

    /// Directions due to repeat at `now`.
    pub fn due(&mut self, now: Instant) -> Vec<Pad> {
        let mut due = Vec::new();
        for (pad, next) in &mut self.held {
            if now >= *next {
                due.push(*pad);
                *next = now + Self::INTERVAL;
            }
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_action_and_never_clash_within_a_context() {
        let bindings = Bindings::default();
        for context in [Context::Browse, Context::Player] {
            for pad in Pad::ALL {
                let actions: Vec<Action> = Action::ALL
                    .into_iter()
                    .filter(|a| a.context() == context && bindings.buttons(*a).contains(&pad))
                    .collect();
                assert!(actions.len() <= 1, "{pad:?} in {context:?}: {actions:?}");
            }
        }
        assert_eq!(
            bindings.action_for(Pad::A, Context::Browse),
            Some(Action::Activate)
        );
        assert_eq!(
            bindings.action_for(Pad::A, Context::Player),
            Some(Action::PlayPause)
        );
        assert_eq!(
            bindings.action_for(Pad::Left, Context::Player),
            Some(Action::SeekBack)
        );
    }

    #[test]
    fn remapping_steals_the_button_from_its_previous_action() {
        let mut bindings = Bindings::default();
        bindings.set(Action::Search, Pad::Y);
        assert_eq!(
            bindings.action_for(Pad::Y, Context::Browse),
            Some(Action::Search)
        );
        assert!(bindings.buttons(Action::Home).is_empty());
        // Player bindings for Y are untouched.
        assert_eq!(
            bindings.action_for(Pad::Y, Context::Player),
            Some(Action::SubtitleMenu)
        );
    }

    #[test]
    fn config_round_trip_is_sparse_and_tolerant() {
        let mut bindings = Bindings::default();
        bindings.set(Action::Home, Pad::Select);
        let config = bindings.to_config();
        assert_eq!(config["home"], ["Select"]);
        assert_eq!(config["search"], Vec::<String>::new());
        assert_eq!(Bindings::from_config(&config), bindings);

        let mut hand_edited = config.clone();
        hand_edited.insert("teleport".into(), vec!["A".into()]);
        hand_edited.insert("back".into(), vec!["B".into(), "Banana".into()]);
        let parsed = Bindings::from_config(&hand_edited);
        assert_eq!(parsed.buttons(Action::Back), [Pad::B]);
        // Setting an action back to its default drops it from config.
        let mut reset = parsed.clone();
        reset.set(Action::Home, Pad::Y);
        assert!(!reset.to_config().contains_key("home"));
    }

    #[test]
    fn repeat_waits_then_ticks() {
        let start = Instant::now();
        let mut repeater = Repeater::default();
        repeater.press(Pad::Down, start);
        repeater.press(Pad::A, start); // buttons never repeat
        assert!(repeater.due(start + Duration::from_millis(399)).is_empty());
        assert_eq!(repeater.due(start + Repeater::DELAY), [Pad::Down]);
        assert!(
            repeater
                .due(start + Repeater::DELAY + Duration::from_millis(100))
                .is_empty()
        );
        assert_eq!(
            repeater.due(start + Repeater::DELAY + Repeater::INTERVAL),
            [Pad::Down]
        );
        repeater.release(Pad::Down);
        assert!(repeater.due(start + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn names_round_trip() {
        for pad in Pad::ALL {
            assert_eq!(Pad::from_name(pad.name()), Some(pad));
        }
        assert_eq!(Pad::from_name(" lb "), Some(Pad::LB));
    }
}
