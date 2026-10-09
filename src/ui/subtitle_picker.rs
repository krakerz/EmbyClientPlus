//! Choosing subtitles before playing (details and series pages). The
//! choice is the same per-title one the player remembers (language, and
//! whether only forced subtitles), so it applies to the whole series for
//! episodes; "Default" goes back to Preferences' language.

use adw::prelude::*;
use gtk::{gio, glib};

use crate::emby::models::{BaseItem, MediaStream};
use crate::playback::tracks::{SUBTITLES_OFF, language_name, normalize_language};

const DEFAULT: &str = "default";
const OFF: &str = "off";

/// One menu entry per subtitle language (and forced variant) in `streams`.
fn languages(streams: &[MediaStream]) -> Vec<(String, bool)> {
    let mut found: Vec<(String, bool)> = Vec::new();
    for stream in streams.iter().filter(|s| s.stream_type == "Subtitle") {
        let language = stream
            .language
            .as_deref()
            .map(normalize_language)
            .unwrap_or_else(|| "und".to_string());
        let entry = (language, stream.is_forced);
        if !found.contains(&entry) {
            found.push(entry);
        }
    }
    found
}

fn choice_id(language: &str, forced: bool) -> String {
    format!("{language}:{}", u8::from(forced))
}

fn label_for(language: &str, forced: bool) -> String {
    let name = if language == "und" {
        "Unknown language".to_string()
    } else {
        language_name(language)
    };
    if forced {
        format!("{name} (Forced)")
    } else {
        name
    }
}

/// The current choice for `item`'s title, as a menu state.
fn current(item: &BaseItem) -> String {
    let (key, _) = crate::playback::title_key(item);
    let remembered = crate::playback::remembered_for(&key);
    match remembered
        .as_ref()
        .and_then(|o| o.subtitle_language.as_deref())
    {
        None => DEFAULT.to_string(),
        Some(SUBTITLES_OFF) => OFF.to_string(),
        Some(language) => choice_id(
            &normalize_language(language),
            remembered
                .and_then(|o| o.subtitle_forced_only)
                .unwrap_or(false),
        ),
    }
}

fn button_label(state: &str, entries: &[(String, bool)]) -> String {
    let text = match state {
        DEFAULT => "Default".to_string(),
        OFF => "Off".to_string(),
        id => entries
            .iter()
            .find(|(language, forced)| choice_id(language, *forced) == id)
            .map(|(language, forced)| label_for(language, *forced))
            .unwrap_or_else(|| "Default".to_string()),
    };
    format!("Subtitles: {text}")
}

/// A "Subtitles: …" button for `item` (a movie or an episode with its
/// `media_streams` loaded); `None` when it has no subtitles to pick from.
pub fn button(item: &BaseItem) -> Option<gtk::MenuButton> {
    let entries = languages(&item.media_streams);
    if entries.is_empty() {
        return None;
    }
    let preferred = crate::config::Settings::load()
        .unwrap_or_default()
        .subtitles
        .preferred_language;
    let menu = gio::Menu::new();
    menu.append(
        Some(&format!("Default ({})", language_name(&preferred))),
        Some(&format!("subtitles.choice::{DEFAULT}")),
    );
    menu.append(Some("Off"), Some(&format!("subtitles.choice::{OFF}")));
    let languages_section = gio::Menu::new();
    for (language, forced) in &entries {
        languages_section.append(
            Some(&label_for(language, *forced)),
            Some(&format!(
                "subtitles.choice::{}",
                choice_id(language, *forced)
            )),
        );
    }
    menu.append_section(None, &languages_section);

    let state = current(item);
    let button = gtk::MenuButton::builder()
        .label(button_label(&state, &entries))
        .menu_model(&menu)
        .tooltip_text("Subtitles for this title (the whole series for episodes)")
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let action = gio::SimpleAction::new_stateful(
        "choice",
        Some(glib::VariantTy::STRING),
        &state.to_variant(),
    );
    let (key, item_type) = crate::playback::title_key(item);
    action.connect_activate(glib::clone!(
        #[weak]
        button,
        move |action, value| {
            let Some(choice) = value.and_then(|v| v.get::<String>()) else {
                return;
            };
            action.set_state(&choice.to_variant());
            button.set_label(&button_label(&choice, &entries));
            crate::playback::remember_for(&key, item_type, |o| match choice.as_str() {
                DEFAULT => {
                    o.subtitle_language = None;
                    o.subtitle_forced_only = None;
                }
                OFF => {
                    o.subtitle_language = Some(SUBTITLES_OFF.to_string());
                    o.subtitle_forced_only = Some(false);
                }
                id => {
                    let (language, forced) = id.rsplit_once(':').unwrap_or((id, "0"));
                    o.subtitle_language = Some(language.to_string());
                    o.subtitle_forced_only = Some(forced == "1");
                }
            });
        }
    ));
    let group = gio::SimpleActionGroup::new();
    group.add_action(&action);
    button.insert_action_group("subtitles", Some(&group));
    Some(button)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subtitle(language: &str, forced: bool) -> MediaStream {
        serde_json::from_value(serde_json::json!({
            "Index": 3, "Type": "Subtitle", "Language": language, "IsForced": forced
        }))
        .unwrap()
    }

    #[test]
    fn one_entry_per_language_and_forced() {
        let streams = [
            subtitle("en", false),
            subtitle("eng", false),
            subtitle("eng", true),
            subtitle("jpn", false),
        ];
        assert_eq!(
            languages(&streams),
            [
                ("eng".to_string(), false),
                ("eng".to_string(), true),
                ("jpn".to_string(), false)
            ]
        );
        let entries = languages(&streams);
        assert_eq!(
            button_label("eng:1", &entries),
            "Subtitles: English (Forced)"
        );
        assert_eq!(button_label(OFF, &entries), "Subtitles: Off");
    }
}
