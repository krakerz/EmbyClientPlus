//! Preferences → SVP: SVP Manager's Automatic profile as SVP saved it, and,
//! with "Override SVP settings", values the app writes there before it
//! starts SVP Manager.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::preferences::{choice_row, save};
use crate::config::Settings;
use crate::svp_profile::{self, Current, Masking, Overrides, Preset, Smoothness, Snapshot, Target};

const MISSING_NOTE: &str = "Not in SVP's profile yet; change it once in SVP to override it";
const EXACT_NOTE: &str = "For a fixed target frame rate";
const SMOOTHNESS_NOTE: &str = "SVP's slider, from Higher performance to Higher quality";

#[derive(Clone)]
struct Rows {
    on: adw::SwitchRow,
    target: adw::ComboRow,
    exact: adw::SwitchRow,
    smoothness: adw::ComboRow,
    preset: adw::ComboRow,
    masking: adw::ComboRow,
}

pub(super) fn group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Automatic Profile")
        .description("SVP's settings for videos none of your own SVP profiles match")
        .build();
    let snapshot = match load() {
        Ok(snapshot) => Rc::new(snapshot),
        Err((title, detail)) => {
            group.add(
                &adw::ActionRow::builder()
                    .title(title)
                    .subtitle(glib::markup_escape_text(&detail).as_str())
                    .build(),
            );
            return group;
        }
    };
    let chosen = Settings::load().unwrap_or_default().frame_gen.svp_override;
    let loading = Rc::new(Cell::new(false));

    let override_row = adw::SwitchRow::builder()
        .title("Override SVP settings")
        .subtitle(override_subtitle(chosen.enabled))
        .active(chosen.enabled)
        .build();
    let rows = Rows {
        on: adw::SwitchRow::builder()
            .title("Do frame rate conversion")
            .build(),
        target: combo(
            "Target frame rate",
            &Target::choices(),
            Target::label,
            &snapshot.target,
        ),
        exact: adw::SwitchRow::builder()
            .title("Force exact frame rate")
            .build(),
        smoothness: combo(
            "Performance \u{2194} quality",
            &Smoothness::choices(),
            Smoothness::label,
            &snapshot.smoothness,
        ),
        preset: combo(
            "Optimized for",
            &Preset::ALL,
            |p| p.label().to_string(),
            &snapshot.preset,
        ),
        masking: combo(
            "Artifacts masking",
            &Masking::ALL,
            |m| m.label().to_string(),
            &snapshot.masking,
        ),
    };
    show(&rows, &snapshot, &chosen, &loading);

    connect_switch(&rows.on, &loading, |o, v| o.on = v);
    connect_switch(&rows.exact, &loading, |o, v| o.exact = v);
    connect_choice(&rows.target, Target::choices(), &loading, |o, v| {
        o.target = v
    });
    connect_choice(&rows.smoothness, Smoothness::choices(), &loading, |o, v| {
        o.smoothness = v
    });
    connect_choice(&rows.preset, Preset::ALL.to_vec(), &loading, |o, v| {
        o.preset = v
    });
    connect_choice(&rows.masking, Masking::ALL.to_vec(), &loading, |o, v| {
        o.masking = v
    });

    // Weak refs: a strong `Rows` here would be an Rc cycle via the group.
    let (on, target, exact) = (
        rows.on.downgrade(),
        rows.target.downgrade(),
        rows.exact.downgrade(),
    );
    let (smoothness, preset, masking) = (
        rows.smoothness.downgrade(),
        rows.preset.downgrade(),
        rows.masking.downgrade(),
    );
    {
        let snapshot = snapshot.clone();
        let loading = loading.clone();
        override_row.connect_active_notify(move |row| {
            let enabled = row.is_active();
            save(|s| s.frame_gen.svp_override.enabled = enabled);
            row.set_subtitle(override_subtitle(enabled));
            let (
                Some(on),
                Some(target),
                Some(exact),
                Some(smoothness),
                Some(preset),
                Some(masking),
            ) = (
                on.upgrade(),
                target.upgrade(),
                exact.upgrade(),
                smoothness.upgrade(),
                preset.upgrade(),
                masking.upgrade(),
            )
            else {
                return;
            };
            let chosen = Settings::load().unwrap_or_default().frame_gen.svp_override;
            let rows = Rows {
                on,
                target,
                exact,
                smoothness,
                preset,
                masking,
            };
            show(&rows, &snapshot, &chosen, &loading);
        });
    }

    group.add(&override_row);
    group.add(&rows.on);
    group.add(&rows.target);
    group.add(&rows.exact);
    group.add(&rows.smoothness);
    group.add(&rows.preset);
    group.add(&rows.masking);
    if !snapshot.own_profiles.is_empty() {
        let names = snapshot
            .own_profiles
            .iter()
            .map(|n| format!("“{n}”"))
            .collect::<Vec<_>>()
            .join(", ");
        group.add(
            &adw::ActionRow::builder()
                .title("Your own SVP profiles")
                .subtitle(
                    glib::markup_escape_text(&format!(
                        "{names}: SVP uses these instead for the videos they match"
                    ))
                    .as_str(),
                )
                .build(),
        );
    }
    group
}

/// The snapshot, or what to show instead (title, detail).
fn load() -> Result<Snapshot, (&'static str, String)> {
    let Some(path) = svp_profile::profiles_path() else {
        let searched: Vec<String> = svp_profile::candidates()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let detail = if searched.is_empty() {
            "No home folder".to_string()
        } else {
            format!("Looked for {}", searched.join(", "))
        };
        return Err(("SVP settings not found", detail));
    };
    std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| svp_profile::read(&text).map_err(|e| e.to_string()))
        .map_err(|e| {
            (
                "SVP settings couldn't be read",
                format!("{} — {e}", path.display()),
            )
        })
}

fn override_subtitle(enabled: bool) -> &'static str {
    if !enabled {
        return "Off: shows SVP's settings and never changes them";
    }
    if !Settings::load()
        .unwrap_or_default()
        .frame_gen
        .auto_start_svp()
    {
        "Applied only when this app starts SVP Manager: turn on Start SVP Manager automatically"
    } else {
        // SVP Manager reads its profiles only at startup, and the app starts
        // it at launch (Game Mode relies on that).
        "Restart Emby Client+ after changing these to apply them"
    }
}

/// A drop-down of `choices`, plus SVP's own value when it's none of them
/// (that extra entry sits at `choices.len()` and means "keep SVP's").
fn combo<T: Copy>(
    title: &str,
    choices: &[T],
    label: impl Fn(T) -> String,
    current: &Current<T>,
) -> adw::ComboRow {
    let mut labels: Vec<String> = choices.iter().map(|c| label(*c)).collect();
    match current {
        Current::Custom(raw) => labels.push(format!("Custom ({raw})")),
        Current::Missing => labels.push("SVP's default".to_string()),
        Current::Known(_) => {}
    }
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    choice_row(title, &refs, 0)
}

/// Saves a switch's value while the override is shown as editable.
fn connect_switch(
    row: &adw::SwitchRow,
    loading: &Rc<Cell<bool>>,
    store: fn(&mut Overrides, Option<bool>),
) {
    let loading = loading.clone();
    row.connect_active_notify(move |row| {
        if loading.get() {
            return;
        }
        let value = Some(row.is_active());
        save(|s| store(&mut s.frame_gen.svp_override, value));
    });
}

/// Saves a drop-down's choice; the extra "Custom"/"SVP's default" entry
/// saves `None` (keep SVP's value).
fn connect_choice<T: Copy + 'static>(
    row: &adw::ComboRow,
    choices: Vec<T>,
    loading: &Rc<Cell<bool>>,
    store: fn(&mut Overrides, Option<T>),
) {
    let loading = loading.clone();
    row.connect_selected_notify(move |row| {
        if loading.get() {
            return;
        }
        let value = choices.get(row.selected() as usize).copied();
        save(|s| store(&mut s.frame_gen.svp_override, value));
    });
}

/// SVP's values, or with the override on the chosen ones where set.
fn show(rows: &Rows, snapshot: &Snapshot, chosen: &Overrides, loading: &Cell<bool>) {
    loading.set(true);
    let edit = chosen.enabled;
    show_switch(&rows.on, &snapshot.on, chosen.on, edit, "");
    show_switch(&rows.exact, &snapshot.exact, chosen.exact, edit, EXACT_NOTE);
    show_choice(
        &rows.target,
        &Target::choices(),
        &snapshot.target,
        chosen.target,
        edit,
        "",
    );
    show_choice(
        &rows.smoothness,
        &Smoothness::choices(),
        &snapshot.smoothness,
        chosen.smoothness,
        edit,
        SMOOTHNESS_NOTE,
    );
    show_choice(
        &rows.preset,
        &Preset::ALL,
        &snapshot.preset,
        chosen.preset,
        edit,
        "",
    );
    show_choice(
        &rows.masking,
        &Masking::ALL,
        &snapshot.masking,
        chosen.masking,
        edit,
        "",
    );
    loading.set(false);
}

fn show_switch(
    row: &adw::SwitchRow,
    current: &Current<bool>,
    chosen: Option<bool>,
    edit: bool,
    default_subtitle: &str,
) {
    let value = chosen.filter(|_| edit).or(current.known());
    row.set_active(value.unwrap_or(false));
    row.set_sensitive(edit && !current.is_missing());
    let note = match current {
        Current::Missing => MISSING_NOTE.to_string(),
        Current::Custom(raw) => format!("SVP has {raw}"),
        Current::Known(_) => default_subtitle.to_string(),
    };
    row.set_subtitle(glib::markup_escape_text(&note).as_str());
}

fn show_choice<T: Copy + PartialEq>(
    row: &adw::ComboRow,
    choices: &[T],
    current: &Current<T>,
    chosen: Option<T>,
    edit: bool,
    default_subtitle: &str,
) {
    let value = chosen.filter(|_| edit).or(current.known());
    // `None` only when SVP's value isn't Known, and then `combo` added the
    // extra entry at `choices.len()`.
    let at = value
        .and_then(|v| choices.iter().position(|c| *c == v))
        .unwrap_or(choices.len());
    row.set_selected(at as u32);
    row.set_sensitive(edit && !current.is_missing());
    row.set_subtitle(if current.is_missing() {
        MISSING_NOTE
    } else {
        default_subtitle
    });
}
