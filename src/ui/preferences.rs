//! Preferences: global defaults for playback, plus housekeeping. Each
//! change is written to config.toml right away.

use adw::prelude::*;
use gtk::{gio, glib};

use crate::config::{FrameGenBackend, Settings};
use crate::db::Db;
use crate::playback::tracks::normalize_language;
use crate::playback::{QUALITIES, Quality};

/// Languages offered in the pickers (ISO 639-2, display name).
const LANGUAGES: [(&str, &str); 16] = [
    ("eng", "English"),
    ("jpn", "Japanese"),
    ("zho", "Chinese"),
    ("kor", "Korean"),
    ("ind", "Indonesian"),
    ("msa", "Malay"),
    ("tha", "Thai"),
    ("vie", "Vietnamese"),
    ("spa", "Spanish"),
    ("por", "Portuguese"),
    ("fra", "French"),
    ("deu", "German"),
    ("ita", "Italian"),
    ("rus", "Russian"),
    ("ara", "Arabic"),
    ("nld", "Dutch"),
];

pub fn show(parent: &impl IsA<gtk::Widget>) {
    let settings = Settings::load().unwrap_or_default();
    let dialog = adw::PreferencesDialog::new();
    let page = adw::PreferencesPage::new();
    dialog.add(&page);

    // Playback.
    let playback = adw::PreferencesGroup::builder()
        .title("Playback")
        .description(
            "Defaults for new playback; the player's menus change them per title or session.",
        )
        .build();
    let svp = adw::SwitchRow::builder()
        .title("Use SVP by default")
        .subtitle("Titles where you toggled SVP in the player keep their own choice")
        .active(settings.frame_gen.default_backend == FrameGenBackend::Svp)
        .build();
    svp.connect_active_notify(|row| {
        let backend = if row.is_active() {
            FrameGenBackend::Svp
        } else {
            FrameGenBackend::Off
        };
        save(|s| s.frame_gen.default_backend = backend);
    });
    playback.add(&svp);

    let labels: Vec<String> = QUALITIES.iter().map(|q| q.label()).collect();
    let quality = adw::ComboRow::builder()
        .title("Default quality")
        .subtitle("Anything below Original asks the server to transcode")
        .model(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .build();
    let current = Quality::from_mbps(settings.playback.bitrate_cap_mbps);
    quality.set_selected(QUALITIES.iter().position(|q| *q == current).unwrap_or(0) as u32);
    quality.connect_selected_notify(|row| {
        if let Some(chosen) = QUALITIES.get(row.selected() as usize) {
            let mbps = chosen.mbps();
            save(|s| s.playback.bitrate_cap_mbps = mbps);
        }
    });
    playback.add(&quality);
    page.add(&playback);

    // Languages.
    let languages = adw::PreferencesGroup::builder()
        .title("Languages")
        .description("Used when a title has no remembered choice")
        .build();
    let audio = language_row("Audio", &settings.audio.preferred_language);
    audio.connect_selected_notify(|row| {
        if let Some(code) = selected_language(row) {
            save(|s| s.audio.preferred_language = code);
        }
    });
    let subtitles = language_row("Subtitles", &settings.subtitles.preferred_language);
    subtitles.connect_selected_notify(|row| {
        if let Some(code) = selected_language(row) {
            save(|s| s.subtitles.preferred_language = code);
        }
    });
    languages.add(&audio);
    languages.add(&subtitles);
    page.add(&languages);

    // Remembered choices.
    let remembered = adw::PreferencesGroup::builder()
        .title("Remembered Choices")
        .build();
    let forget = adw::ActionRow::builder()
        .title("Per-title choices")
        .subtitle("Audio, subtitle and SVP picks remembered for each series or movie")
        .build();
    let forget_button = gtk::Button::builder()
        .label("Forget All")
        .valign(gtk::Align::Center)
        .css_classes(["destructive-action"])
        .build();
    forget_button.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            let message = match Db::open_default().and_then(|db| db.clear_overrides()) {
                Ok(0) => "Nothing was remembered".to_string(),
                Ok(1) => "Forgot 1 title's choices".to_string(),
                Ok(n) => format!("Forgot {n} titles' choices"),
                Err(e) => {
                    tracing::warn!("{e:#}");
                    format!("Couldn't clear them: {e}")
                }
            };
            dialog.add_toast(adw::Toast::new(&message));
        }
    ));
    forget.add_suffix(&forget_button);
    remembered.add(&forget);
    page.add(&remembered);

    // Diagnostics.
    let diagnostics = adw::PreferencesGroup::builder()
        .title("Diagnostics")
        .build();
    if let Some(logs) = crate::logging::log_dir() {
        let row = adw::ActionRow::builder()
            .title("Logs")
            .subtitle(logs.display().to_string())
            .subtitle_selectable(true)
            .build();
        let open = gtk::Button::builder()
            .label("Open Folder")
            .valign(gtk::Align::Center)
            .build();
        open.connect_clicked(move |_| {
            let uri = gio::File::for_path(&logs).uri();
            if let Err(e) =
                gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
            {
                tracing::warn!("could not open {uri}: {e}");
            }
        });
        row.add_suffix(&open);
        diagnostics.add(&row);
    }
    page.add(&diagnostics);

    dialog.present(Some(parent));
}

/// A language picker preselected on `current`; an unlisted configured
/// code is kept as an extra entry so opening the dialog never changes it.
fn language_row(title: &str, current: &str) -> adw::ComboRow {
    let current = normalize_language(current);
    let mut codes: Vec<String> = LANGUAGES.iter().map(|(code, _)| code.to_string()).collect();
    let mut names: Vec<String> = LANGUAGES.iter().map(|(_, name)| name.to_string()).collect();
    if !codes.contains(&current) {
        codes.push(current.clone());
        names.push(current.clone());
    }
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .build();
    row.set_selected(codes.iter().position(|c| *c == current).unwrap_or(0) as u32);
    // SAFETY: only ever stored and read as `Vec<String>` under this key.
    unsafe { row.set_data(CODES_KEY, codes) };
    row
}

const CODES_KEY: &str = "embyclientplus-language-codes";

fn selected_language(row: &adw::ComboRow) -> Option<String> {
    // SAFETY: see `language_row`.
    let codes = unsafe { row.data::<Vec<String>>(CODES_KEY)?.as_ref() };
    codes.get(row.selected() as usize).cloned()
}

fn save(change: impl FnOnce(&mut Settings)) {
    if let Err(e) = Settings::update(change) {
        tracing::warn!("could not save preferences: {e:#}");
    }
}
