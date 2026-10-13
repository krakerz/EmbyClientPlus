//! The signed-in user's own account (Emby's per-user settings, shared with
//! Emby's apps): picture, password, PIN, playback and display choices, and
//! how libraries show on Home. Nothing that needs admin rights. Changes go
//! to the server as they're made; see `account` for how they apply.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use serde_json::{Map, Value};

use super::Ui;
use super::preferences::{LANGUAGES, choice_row};
use crate::config::{IntroSkip, SubtitleMode};
use crate::emby::models::BaseItem;
use crate::playback::tracks::normalize_language;
use crate::runtime::spawn_tokio;

const AVATAR_SIZE: i32 = 96;

/// The header's avatar button: opens the Profile.
pub fn button(ui: &Ui) -> gtk::Button {
    let avatar = adw::Avatar::builder().size(26).show_initials(true).build();
    let button = gtk::Button::builder()
        .child(&avatar)
        .tooltip_text("Profile")
        .css_classes(["flat", "circular"])
        .build();
    // The server's current name and picture (the saved copy may be old).
    let (client, user_id, weak) = (ui.client(), ui.user_id(), ui.downgrade());
    let avatar_ref = avatar.downgrade();
    glib::spawn_future_local(async move {
        let Ok(user) = spawn_tokio(async move { client.user(&user_id).await }).await else {
            return;
        };
        if let (Some(ui), Some(avatar)) = (weak.upgrade(), avatar_ref.upgrade()) {
            avatar.set_text(user.get("Name").and_then(Value::as_str));
            load_picture(&ui, &avatar, &user);
        }
    });
    let weak = ui.downgrade();
    button.connect_clicked(move |button| {
        if let Some(ui) = weak.upgrade() {
            show(&ui, button);
        }
    });
    button
}

pub fn show(ui: &Ui, parent: &impl IsA<gtk::Widget>) {
    let Some(user) = crate::account::user() else {
        return;
    };
    let config = user
        .get("Configuration")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let dialog = adw::PreferencesDialog::builder()
        .title("Profile")
        .search_enabled(false)
        .build();
    let page = adw::PreferencesPage::builder()
        .title("Profile")
        .icon_name(super::icons::PERSON)
        .build();
    page.add(&picture_group(ui, &user));
    page.add(&security_group(ui, &user));
    page.add(&playback_group(ui, &config));
    page.add(&display_group(ui, &config));
    page.add(&libraries_group(ui, &config));
    dialog.add(&page);

    // Home picks up library order and visibility changes.
    let parent_widget = parent.as_ref().clone();
    dialog.connect_closed(move |_| {
        let _ = parent_widget.activate_action("home.refresh", None);
    });
    dialog.present(Some(parent));
}

/// Changes the account's configuration and saves it to the server.
fn save(ui: &Ui, edit: impl FnOnce(&mut Map<String, Value>)) {
    let Some((user_id, configuration)) = crate::account::change(edit) else {
        return;
    };
    let client = ui.client();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let result =
            spawn_tokio(async move { client.save_configuration(&user_id, &configuration).await })
                .await;
        if let (Err(e), Some(ui)) = (result, weak.upgrade()) {
            ui.report_error("Couldn't save your settings", &e);
        }
    });
}

fn text(config: &Map<String, Value>, key: &str) -> String {
    config
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn flag(config: &Map<String, Value>, key: &str, default: bool) -> bool {
    config.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// A switch saved to `key` of the configuration.
fn switch_row(
    ui: &Ui,
    config: &Map<String, Value>,
    key: &'static str,
    default: bool,
    title: &str,
    subtitle: Option<&str>,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .active(flag(config, key, default))
        .build();
    if let Some(subtitle) = subtitle {
        row.set_subtitle(subtitle);
    }
    let ui = ui.downgrade();
    row.connect_active_notify(move |row| {
        let on = row.is_active();
        if let Some(ui) = ui.upgrade() {
            save(&ui, |c| {
                c.insert(key.into(), on.into());
            });
        }
    });
    row
}

// Account: picture, name, password, PIN, sign out.

/// The picture and name. Its own group: a group puts plain widgets after
/// its rows, so sharing one with rows would push it below them.
fn picture_group(ui: &Ui, user: &Value) -> adw::PreferencesGroup {
    let name = user
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let group = adw::PreferencesGroup::new();

    let avatar = adw::Avatar::builder()
        .size(AVATAR_SIZE)
        .text(&name)
        .show_initials(true)
        .halign(gtk::Align::Center)
        .build();
    load_picture(ui, &avatar, user);
    let title = gtk::Label::builder()
        .label(&name)
        .css_classes(["title-2"])
        .build();
    let change = gtk::Button::builder()
        .label("Change Picture…")
        .css_classes(["pill"])
        .build();
    let remove = gtk::Button::builder()
        .label("Remove")
        .css_classes(["pill"])
        .visible(user.get("PrimaryImageTag").is_some())
        .build();
    let buttons = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    buttons.append(&change);
    buttons.append(&remove);
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_bottom(12)
        .build();
    header.append(&avatar);
    header.append(&title);
    header.append(&buttons);
    group.add(&header);

    change.connect_clicked({
        let (ui, avatar, remove) = (ui.downgrade(), avatar.clone(), remove.clone());
        move |button| {
            let Some(ui) = ui.upgrade() else { return };
            choose_picture(&ui, button, &avatar, &remove);
        }
    });
    remove.connect_clicked({
        let (ui, avatar) = (ui.downgrade(), avatar.clone());
        move |button| {
            let Some(ui) = ui.upgrade() else { return };
            let (client, user_id) = (ui.client(), ui.user_id());
            let (weak, avatar, button) = (ui.downgrade(), avatar.clone(), button.clone());
            glib::spawn_future_local(async move {
                let result =
                    spawn_tokio(async move { client.delete_user_picture(&user_id).await }).await;
                match (result, weak.upgrade()) {
                    (Ok(()), _) => {
                        avatar.set_custom_image(None::<&gdk::Paintable>);
                        button.set_visible(false);
                    }
                    (Err(e), Some(ui)) => ui.report_error("Couldn't remove the picture", &e),
                    _ => {}
                }
            });
        }
    });

    group
}

/// Password and PIN, above the settings. Signing out is in Home's menu.
fn security_group(ui: &Ui, user: &Value) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.add(&password_row(ui));
    group.add(&pin_row(ui, user));
    group
}

pub(super) fn load_picture(ui: &Ui, avatar: &adw::Avatar, user: &Value) {
    let Some(tag) = user.get("PrimaryImageTag").and_then(Value::as_str) else {
        return;
    };
    let (client, user_id) = (ui.client(), ui.user_id());
    let path = format!(
        "/emby/Users/{user_id}/Images/Primary?tag={tag}&maxWidth={}",
        AVATAR_SIZE * 2
    );
    let avatar = avatar.downgrade();
    glib::spawn_future_local(async move {
        let Ok(bytes) = spawn_tokio(async move { client.fetch_bytes(&path).await }).await else {
            return;
        };
        if let (Ok(texture), Some(avatar)) = (
            gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)),
            avatar.upgrade(),
        ) {
            avatar.set_custom_image(Some(&texture));
        }
    });
}

fn choose_picture(ui: &Ui, anchor: &gtk::Button, avatar: &adw::Avatar, remove: &gtk::Button) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Pictures"));
    for mime in ["image/jpeg", "image/png", "image/webp"] {
        filter.add_mime_type(mime);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let chooser = gtk::FileDialog::builder()
        .title("Choose a picture")
        .filters(&filters)
        .build();
    let window = anchor.root().and_downcast::<gtk::Window>();
    let (weak, avatar, remove) = (ui.downgrade(), avatar.clone(), remove.clone());
    chooser.open(window.as_ref(), gio::Cancellable::NONE, move |file| {
        let Some(path) = file.ok().and_then(|f| f.path()) else {
            return;
        };
        let Some(ui) = weak.upgrade() else { return };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => return ui.report_error("Couldn't read the picture", &e.into()),
        };
        let mime = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("webp") => "image/webp",
            _ => "image/jpeg",
        };
        let texture = gdk::Texture::from_bytes(&glib::Bytes::from(&bytes)).ok();
        let (client, user_id) = (ui.client(), ui.user_id());
        let weak = ui.downgrade();
        glib::spawn_future_local(async move {
            let result =
                spawn_tokio(async move { client.set_user_picture(&user_id, &bytes, mime).await })
                    .await;
            match (result, weak.upgrade()) {
                (Ok(()), _) => {
                    avatar.set_custom_image(texture.as_ref());
                    remove.set_visible(true);
                }
                (Err(e), Some(ui)) => ui.report_error("Couldn't set the picture", &e),
                _ => {}
            }
        });
    });
}

fn password_row(ui: &Ui) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder().title("Change Password").build();
    let current = adw::PasswordEntryRow::builder()
        .title("Current password")
        .build();
    let new = adw::PasswordEntryRow::builder()
        .title("New password")
        .build();
    let again = adw::PasswordEntryRow::builder()
        .title("New password again")
        .build();
    let apply = adw::ButtonRow::builder()
        .title("Change Password")
        .css_classes(["suggested-action"])
        .build();
    for child in [
        current.upcast_ref::<gtk::Widget>(),
        new.upcast_ref(),
        again.upcast_ref(),
        apply.upcast_ref(),
    ] {
        row.add_row(child);
    }
    let fields = Rc::new((current, new, again));
    apply.connect_activated({
        let (ui, row) = (ui.downgrade(), row.clone());
        move |_| {
            let Some(ui) = ui.upgrade() else { return };
            let (current, new, again) = &*fields;
            if new.text() != again.text() {
                ui.toast("The new passwords don't match");
                return;
            }
            let (current_pw, new_pw) = (current.text().to_string(), new.text().to_string());
            let (client, user_id) = (ui.client(), ui.user_id());
            let (weak, row, fields) = (ui.downgrade(), row.clone(), fields.clone());
            glib::spawn_future_local(async move {
                let result = spawn_tokio(async move {
                    client.change_password(&user_id, &current_pw, &new_pw).await
                })
                .await;
                let Some(ui) = weak.upgrade() else { return };
                match result {
                    Ok(()) => {
                        for field in [&fields.0, &fields.1, &fields.2] {
                            field.set_text("");
                        }
                        row.set_expanded(false);
                        ui.toast("Password changed");
                    }
                    Err(e) => ui.report_error(
                        "Couldn't change the password (is the current one right?)",
                        &e,
                    ),
                }
            });
        }
    });
    row
}

fn pin_row(ui: &Ui, user: &Value) -> adw::PasswordEntryRow {
    let has_pin = user
        .get("Configuration")
        .and_then(|c| c.get("ProfilePin"))
        .and_then(Value::as_str)
        .is_some_and(|pin| !pin.is_empty());
    let row = adw::PasswordEntryRow::builder()
        .title(if has_pin {
            "Profile PIN (set; enter a new one, or clear it)"
        } else {
            "Profile PIN"
        })
        .show_apply_button(true)
        .build();
    let ui = ui.downgrade();
    row.connect_apply(move |row| {
        let pin = row.text().trim().to_string();
        if !pin.chars().all(|c| c.is_ascii_digit()) {
            if let Some(ui) = ui.upgrade() {
                ui.toast("A PIN is digits only");
            }
            return;
        }
        if let Some(ui) = ui.upgrade() {
            save(&ui, |c| {
                c.insert(
                    "ProfilePin".into(),
                    if pin.is_empty() {
                        Value::Null
                    } else {
                        pin.clone().into()
                    },
                );
            });
            ui.toast(if pin.is_empty() {
                "PIN removed"
            } else {
                "PIN set"
            });
        }
        row.set_text("");
    });
    row
}

// Playback.

/// A language drop-down with "No preference" (Emby's empty value) first.
fn language_choice(
    ui: &Ui,
    config: &Map<String, Value>,
    key: &'static str,
    title: &str,
) -> adw::ComboRow {
    let current = text(config, key);
    let current = if current.is_empty() {
        String::new()
    } else {
        normalize_language(&current)
    };
    let mut codes: Vec<String> = vec![String::new()];
    let mut names: Vec<String> = vec!["No preference".into()];
    for (code, name) in LANGUAGES {
        codes.push(code.to_string());
        names.push(name.to_string());
    }
    if !codes.contains(&current) {
        codes.push(current.clone());
        names.push(current.clone());
    }
    let labels: Vec<&str> = names.iter().map(String::as_str).collect();
    let row = choice_row(
        title,
        &labels,
        codes.iter().position(|c| *c == current).unwrap_or(0),
    );
    let ui = ui.downgrade();
    row.connect_selected_notify(move |row| {
        let Some(code) = codes.get(row.selected() as usize).cloned() else {
            return;
        };
        if let Some(ui) = ui.upgrade() {
            save(&ui, |c| {
                c.insert(key.into(), code.into());
            });
        }
    });
    row
}

/// A drop-down over an enum Emby stores by name.
fn enum_choice<T: Copy + PartialEq + serde::Serialize + serde::de::DeserializeOwned + 'static>(
    ui: &Ui,
    config: &Map<String, Value>,
    key: &'static str,
    title: &str,
    all: &'static [T],
    label: fn(T) -> &'static str,
    default: T,
) -> adw::ComboRow {
    let current: T = config
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(default);
    let labels: Vec<&str> = all.iter().map(|v| label(*v)).collect();
    let row = choice_row(
        title,
        &labels,
        all.iter().position(|v| *v == current).unwrap_or(0),
    );
    let ui = ui.downgrade();
    row.connect_selected_notify(move |row| {
        let Some(value) = all.get(row.selected() as usize) else {
            return;
        };
        let Ok(value) = serde_json::to_value(value) else {
            return;
        };
        if let Some(ui) = ui.upgrade() {
            save(&ui, |c| {
                c.insert(key.into(), value);
            });
        }
    });
    row
}

fn playback_group(ui: &Ui, config: &Map<String, Value>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Playback")
        .description("Saved to your Emby account; Emby's own apps use them too")
        .build();
    group.add(&language_choice(
        ui,
        config,
        "AudioLanguagePreference",
        "Audio language",
    ));
    group.add(&switch_row(
        ui,
        config,
        "PlayDefaultAudioTrack",
        true,
        "Play the default audio track",
        Some("The file's own choice, whatever its language"),
    ));
    group.add(&language_choice(
        ui,
        config,
        "SubtitleLanguagePreference",
        "Subtitle language",
    ));
    group.add(&enum_choice(
        ui,
        config,
        "SubtitleMode",
        "Subtitles",
        &SubtitleMode::ALL,
        SubtitleMode::label,
        SubtitleMode::Default,
    ));
    group.add(&switch_row(
        ui,
        config,
        "RememberAudioSelections",
        true,
        "Remember audio choices",
        Some("Per series or movie"),
    ));
    group.add(&switch_row(
        ui,
        config,
        "RememberSubtitleSelections",
        true,
        "Remember subtitle choices",
        Some("Per series or movie"),
    ));
    group.add(&switch_row(
        ui,
        config,
        "EnableNextEpisodeAutoPlay",
        true,
        "Play the next episode automatically",
        None,
    ));
    let rewind = adw::SpinRow::with_range(0.0, 60.0, 1.0);
    rewind.set_title("Rewind on resume");
    rewind.set_subtitle("Seconds to go back when resuming");
    rewind.set_value(
        config
            .get("ResumeRewindSeconds")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    );
    let weak = ui.downgrade();
    rewind.connect_value_notify(move |row| {
        let seconds = row.value() as i64;
        if let Some(ui) = weak.upgrade() {
            save(&ui, |c| {
                c.insert("ResumeRewindSeconds".into(), seconds.into());
            });
        }
    });
    group.add(&rewind);
    group.add(&enum_choice(
        ui,
        config,
        "IntroSkipMode",
        "Intros",
        &IntroSkip::ALL,
        IntroSkip::label,
        IntroSkip::ShowButton,
    ));
    group
}

// Display.

fn display_group(ui: &Ui, config: &Map<String, Value>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Display").build();
    for (key, title) in [
        ("HidePlayedInLatest", "Hide watched items in Latest"),
        (
            "HidePlayedInMoreLikeThis",
            "Hide watched items in More Like This",
        ),
        (
            "HidePlayedInSuggestions",
            "Hide watched items in Suggestions",
        ),
    ] {
        group.add(&switch_row(ui, config, key, false, title, None));
    }
    group
}

// Libraries: order, and whether each shows on Home and in Latest.

fn view_key(view: &BaseItem) -> String {
    view.guid.clone().unwrap_or_else(|| view.id.clone())
}

fn listed(list: &[String], view: &BaseItem) -> bool {
    list.iter()
        .any(|id| *id == view.id || view.guid.as_deref() == Some(id.as_str()))
}

fn strings(config: &Map<String, Value>, key: &str) -> Vec<String> {
    config
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn libraries_group(ui: &Ui, config: &Map<String, Value>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Libraries")
        .description("Their order on Home, and whether they show there (with their Latest row)")
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    group.add(&list);
    let config = config.clone();
    let (client, user_id) = (ui.client(), ui.user_id());
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        // The user's views leave out libraries hidden from Home; the media
        // folders still list them (but not channels like Trailers).
        let (views, folders) =
            spawn_tokio(
                async move { tokio::join!(client.views(&user_id), client.media_folders()) },
            )
            .await;
        let Ok(mut views) = views else { return };
        let same =
            |a: &BaseItem, b: &BaseItem| a.id == b.id || (a.guid.is_some() && a.guid == b.guid);
        for folder in folders.unwrap_or_default() {
            if !views.iter().any(|v| same(v, &folder)) {
                views.push(folder);
            }
        }
        let Some(ui) = weak.upgrade() else { return };
        let settings: crate::emby::models::ViewSettings =
            serde_json::from_value(Value::Object(config.clone())).unwrap_or_default();
        settings.sort(&mut views);
        views.retain(|v| v.collection_type.as_deref() != Some("livetv"));
        let views = Rc::new(std::cell::RefCell::new(views));
        fill_libraries(&ui, &list, &views, &config);
    });
    group
}

fn fill_libraries(
    ui: &Ui,
    list: &gtk::ListBox,
    views: &Rc<std::cell::RefCell<Vec<BaseItem>>>,
    config: &Map<String, Value>,
) {
    list.remove_all();
    let hidden = strings(config, "MyMediaExcludes");
    let count = views.borrow().len();
    for (index, view) in views.borrow().iter().enumerate() {
        let row = adw::ActionRow::builder().title(&view.name).build();
        let on_home = gtk::Switch::builder()
            .active(!listed(&hidden, view))
            .valign(gtk::Align::Center)
            .tooltip_text("Show on Home")
            .build();
        row.add_suffix(&on_home);
        for (icon, tooltip, step, enabled) in [
            (super::icons::MOVE_UP, "Move up", -1, index > 0),
            (super::icons::MOVE_DOWN, "Move down", 1, index + 1 < count),
        ] {
            let button = gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .valign(gtk::Align::Center)
                .sensitive(enabled)
                .css_classes(["flat"])
                .build();
            let (weak, list, views) = (ui.downgrade(), list.clone(), views.clone());
            button.connect_clicked(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                let target = index as isize + step;
                views.borrow_mut().swap(index, target as usize);
                let order: Vec<Value> = views.borrow().iter().map(|v| view_key(v).into()).collect();
                save(&ui, |c| {
                    c.insert("OrderedViews".into(), Value::Array(order));
                });
                let config = crate::account::user()
                    .and_then(|u| u.get("Configuration")?.as_object().cloned())
                    .unwrap_or_default();
                fill_libraries(&ui, &list, &views, &config);
            });
            row.add_suffix(&button);
        }
        // One switch: the library's tile and its Latest row together.
        let (weak, key, view) = (ui.downgrade(), view_key(view), view.clone());
        on_home.connect_active_notify(move |switch| {
            let shown = switch.is_active();
            let Some(ui) = weak.upgrade() else { return };
            save(&ui, |c| {
                for list_key in ["MyMediaExcludes", "LatestItemsExcludes"] {
                    let mut entries: Vec<String> = strings(c, list_key)
                        .into_iter()
                        .filter(|id| *id != view.id && view.guid.as_deref() != Some(id.as_str()))
                        .collect();
                    if !shown {
                        entries.push(key.clone());
                    }
                    c.insert(
                        list_key.into(),
                        Value::Array(entries.into_iter().map(Value::from).collect()),
                    );
                }
            });
        });
        list.append(&row);
    }
}
