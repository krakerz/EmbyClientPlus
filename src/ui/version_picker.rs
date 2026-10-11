//! Choosing which version of a title plays (details and series pages), for
//! titles with more than one file (a 4K and a 1080p, say). "Default" is
//! Emby's own pick; the choice lasts while the app runs.

use adw::prelude::*;
use gtk::{gio, glib};

use crate::emby::models::{BaseItem, MediaSource};

const DEFAULT: &str = "default";

fn label_for(source: &MediaSource, position: usize) -> String {
    source
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Version {}", position + 1))
}

fn button_label(state: &str, sources: &[MediaSource]) -> String {
    let text = sources
        .iter()
        .enumerate()
        .find(|(_, source)| source.id == state)
        .map(|(position, source)| label_for(source, position))
        .unwrap_or_else(|| "Default".to_string());
    format!("Version: {text}")
}

/// A "Version: …" button for `item` (fetched with its media sources);
/// `None` when there's only one version.
pub fn button(item: &BaseItem) -> Option<gtk::MenuButton> {
    let sources = item.media_sources.clone();
    if sources.len() < 2 {
        return None;
    }
    let menu = gio::Menu::new();
    menu.append(Some("Default"), Some(&format!("version.choice::{DEFAULT}")));
    let versions = gio::Menu::new();
    for (position, source) in sources.iter().enumerate() {
        versions.append(
            Some(&label_for(source, position)),
            Some(&format!("version.choice::{}", source.id)),
        );
    }
    menu.append_section(None, &versions);

    let state = crate::playback::chosen_version(&item.id).unwrap_or_else(|| DEFAULT.to_string());
    let button = gtk::MenuButton::builder()
        .label(button_label(&state, &sources))
        .menu_model(&menu)
        .tooltip_text("Which version of this title plays")
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let action = gio::SimpleAction::new_stateful(
        "choice",
        Some(glib::VariantTy::STRING),
        &state.to_variant(),
    );
    let item_id = item.id.clone();
    action.connect_activate(glib::clone!(
        #[weak]
        button,
        move |action, value| {
            let Some(choice) = value.and_then(|v| v.get::<String>()) else {
                return;
            };
            action.set_state(&choice.to_variant());
            button.set_label(&button_label(&choice, &sources));
            crate::playback::choose_version(&item_id, (choice != DEFAULT).then_some(choice));
        }
    ));
    let group = gio::SimpleActionGroup::new();
    group.add_action(&action);
    button.insert_action_group("version", Some(&group));
    Some(button)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, name: Option<&str>) -> MediaSource {
        MediaSource {
            id: id.to_string(),
            name: name.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn labels_name_each_version() {
        let sources = [source("a", Some("Movie (4K)")), source("b", None)];
        assert_eq!(button_label("a", &sources), "Version: Movie (4K)");
        assert_eq!(button_label("b", &sources), "Version: Version 2");
        assert_eq!(button_label(DEFAULT, &sources), "Version: Default");
    }
}
