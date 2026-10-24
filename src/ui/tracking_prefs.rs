//! Preferences → Tracking: log in to MyAnimeList, AniList, Kitsu and Trakt,
//! switch each on or off, and the "Also sync unwatched" option.

use std::rc::Rc;
use std::sync::atomic::Ordering;

use adw::prelude::*;
use gtk::glib;

use crate::config::{Settings, TrackingSettings};
use crate::runtime::spawn_tokio_checked;
use crate::tracking::tracker;
use crate::tracking::{Account, LoginInput, ServiceId, oauth, trakt};

pub fn page(_parent: &gtk::Widget) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Tracking")
        .icon_name(crate::ui::icons::WATCHED)
        .build();
    let settings = Settings::load().unwrap_or_default();

    let services = adw::PreferencesGroup::builder()
        .title("Services")
        .description(
            "Finishing an episode (90 %) or marking it watched updates your lists, also for \
             downloads watched offline. Anime are matched through AniBridge; Trakt uses the \
             episode's own IDs.",
        )
        .build();
    for id in ServiceId::ALL {
        let row = adw::ActionRow::builder().title(id.label()).build();
        let suffix = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&suffix);
        refresh_row(&row, &suffix, id);
        services.add(&row);
    }
    page.add(&services);

    let options = adw::PreferencesGroup::builder().title("Options").build();
    let unwatched = adw::SwitchRow::builder()
        .title("Also sync unwatched")
        .subtitle(
            "Marking an episode unwatched lowers its progress on your lists and removes it from \
             Trakt's history. Off: unwatching changes nothing there.",
        )
        .active(settings.tracking.sync_unwatched)
        .build();
    unwatched.connect_active_notify(|row| {
        let value = row.is_active();
        super::preferences::save(|s| s.tracking.sync_unwatched = value);
    });
    options.add(&unwatched);
    page.add(&options);
    page
}

/// Shows the notice on the main window, like the update and sleep timer ones.
fn toast(message: &str) {
    tracker::toast(message);
}

fn set_enabled(tracking: &mut TrackingSettings, id: ServiceId, value: bool) {
    match id {
        ServiceId::Mal => tracking.mal = value,
        ServiceId::AniList => tracking.anilist = value,
        ServiceId::Kitsu => tracking.kitsu = value,
        ServiceId::Trakt => tracking.trakt = value,
    }
}

/// Rebuilds one service's row for its current login state.
fn refresh_row(row: &adw::ActionRow, suffix: &gtk::Box, id: ServiceId) {
    while let Some(child) = suffix.first_child() {
        suffix.remove(&child);
    }
    suffix.set_sensitive(true);
    row.set_activatable_widget(None::<&gtk::Widget>);
    let service = tracker::get().service(id);
    if let Some(account) = service.account().filter(|_| service.is_logged_in()) {
        row.set_subtitle(&glib::markup_escape_text(&format!(
            "Logged in as {}",
            account.name
        )));
        let enabled = id.enabled_in(&Settings::load().unwrap_or_default().tracking);
        let switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(enabled)
            .tooltip_text(format!("Sync to {}", id.label()))
            .build();
        switch.connect_active_notify(move |switch| {
            let value = switch.is_active();
            super::preferences::save(|s| set_enabled(&mut s.tracking, id, value));
            tracker::get().clear_cache();
        });
        let log_out = gtk::Button::builder()
            .label("Log Out")
            .valign(gtk::Align::Center)
            .build();
        log_out.connect_clicked(glib::clone!(
            #[weak]
            row,
            #[weak]
            suffix,
            move |_| log_out_of(id, &row, &suffix)
        ));
        suffix.append(&switch);
        suffix.append(&log_out);
        row.set_activatable_widget(Some(&switch));
        return;
    }
    let log_in = gtk::Button::builder()
        .label("Log In")
        .valign(gtk::Align::Center)
        .build();
    suffix.append(&log_in);
    if id.needs_browser() && crate::gamescope::detected() {
        row.set_subtitle(if crate::auth::tokens_shared_across_modes() {
            "Log in from Desktop Mode; it carries over"
        } else {
            "Log in from Desktop Mode"
        });
        log_in.set_sensitive(false);
        return;
    }
    row.set_subtitle("Not logged in");
    log_in.connect_clicked(glib::clone!(
        #[weak]
        row,
        #[weak]
        suffix,
        move |button| log_in_to(id, button, &row, &suffix)
    ));
    row.set_activatable_widget(Some(&log_in));
}

fn log_out_of(id: ServiceId, row: &adw::ActionRow, suffix: &gtk::Box) {
    suffix.set_sensitive(false);
    let (row, suffix) = (row.downgrade(), suffix.downgrade());
    glib::spawn_future_local(async move {
        let result = spawn_tokio_checked(async move { tracker::get().service(id).logout().await })
            .await
            .and_then(|result| result);
        tracker::get().clear_cache();
        let (Some(row), Some(suffix)) = (row.upgrade(), suffix.upgrade()) else {
            return;
        };
        match result {
            Ok(()) => toast(&format!("Logged out of {}", id.label())),
            Err(e) => toast(&format!("Could not log out of {}: {e}", id.label())),
        }
        refresh_row(&row, &suffix, id);
    });
}

fn log_in_to(id: ServiceId, button: &gtk::Button, row: &adw::ActionRow, suffix: &gtk::Box) {
    match id {
        ServiceId::Mal | ServiceId::AniList => {
            row.set_subtitle("Waiting for the browser…");
            suffix.set_sensitive(false);
            run_login(id, LoginInput::Browser, row, suffix);
        }
        ServiceId::Kitsu => ask_kitsu(button, row, suffix),
        ServiceId::Trakt => ask_trakt(button, row, suffix),
    }
}

/// Runs a login on tokio, then reports it and rebuilds the row.
fn run_login(id: ServiceId, input: LoginInput, row: &adw::ActionRow, suffix: &gtk::Box) {
    let (row, suffix) = (row.downgrade(), suffix.downgrade());
    glib::spawn_future_local(async move {
        let result =
            spawn_tokio_checked(async move { tracker::get().service(id).login(input).await })
                .await
                .and_then(|result| result);
        let (Some(row), Some(suffix)) = (row.upgrade(), suffix.upgrade()) else {
            return;
        };
        finish_login(id, result, &row, &suffix);
    });
}

fn finish_login(
    id: ServiceId,
    result: anyhow::Result<Account>,
    row: &adw::ActionRow,
    suffix: &gtk::Box,
) {
    tracker::get().clear_cache();
    match result {
        Ok(account) => toast(&format!("Logged in to {} as {}", id.label(), account.name)),
        Err(e) => toast(&format!("{} login failed: {e}", id.label())),
    }
    refresh_row(row, suffix, id);
}

/// Log In is enabled once both Kitsu fields are filled.
fn update_ready(user: &adw::EntryRow, password: &adw::PasswordEntryRow, log_in: &adw::ButtonRow) {
    log_in.set_sensitive(!user.text().trim().is_empty() && !password.text().is_empty());
}

/// Kitsu takes a password; it's sent once and never stored.
fn ask_kitsu(anchor: &gtk::Button, row: &adw::ActionRow, suffix: &gtk::Box) {
    let user = adw::EntryRow::builder().title("Email or username").build();
    let password = adw::PasswordEntryRow::builder().title("Password").build();
    let fields = adw::PreferencesGroup::builder()
        .description("Your password is sent to Kitsu once and never stored.")
        .build();
    fields.add(&user);
    fields.add(&password);
    let log_in = super::action_row("Log In", "suggested-action");
    log_in.set_sensitive(false);
    let dialog = super::form_dialog(
        "Log in to Kitsu",
        &[&fields, &super::actions_group(&[&log_in])],
    );
    user.connect_changed(glib::clone!(
        #[weak]
        password,
        #[weak]
        log_in,
        move |user| update_ready(user, &password, &log_in)
    ));
    password.connect_changed(glib::clone!(
        #[weak]
        user,
        #[weak]
        log_in,
        move |password| update_ready(&user, password, &log_in)
    ));
    let apply = Rc::new(glib::clone!(
        #[weak]
        user,
        #[weak]
        password,
        #[weak]
        dialog,
        #[weak]
        row,
        #[weak]
        suffix,
        move || {
            let username = user.text().trim().to_string();
            let secret = password.text().to_string();
            if username.is_empty() || secret.is_empty() {
                return;
            }
            password.set_text("");
            row.set_subtitle("Logging in…");
            suffix.set_sensitive(false);
            run_login(
                ServiceId::Kitsu,
                LoginInput::Password {
                    username,
                    password: secret,
                },
                &row,
                &suffix,
            );
            dialog.close();
        }
    ));
    log_in.connect_activated({
        let apply = apply.clone();
        move |_| apply()
    });
    user.connect_entry_activated({
        let password = password.clone();
        move |_| {
            password.grab_focus();
        }
    });
    password.connect_entry_activated(move |_| apply());
    dialog.present(Some(anchor));
}

/// Trakt shows a code to enter on its site; the login polls until it's used.
fn ask_trakt(anchor: &gtk::Button, row: &adw::ActionRow, suffix: &gtk::Box) {
    row.set_subtitle("Requesting a code…");
    suffix.set_sensitive(false);
    let (anchor, row, suffix) = (anchor.downgrade(), row.downgrade(), suffix.downgrade());
    glib::spawn_future_local(async move {
        let code = spawn_tokio_checked(trakt::request_code())
            .await
            .and_then(|code| code);
        let (Some(anchor), Some(row), Some(suffix)) =
            (anchor.upgrade(), row.upgrade(), suffix.upgrade())
        else {
            return;
        };
        let code = match code {
            Ok(code) => code,
            Err(e) => {
                finish_login(ServiceId::Trakt, Err(e), &row, &suffix);
                return;
            }
        };
        let site = code.verification_url.trim_start_matches("https://");
        let shown = gtk::Label::builder()
            .label(&code.user_code)
            .css_classes(["title-1"])
            .selectable(true)
            .focusable(false)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        let group = adw::PreferencesGroup::builder()
            .description(glib::markup_escape_text(&format!(
                "Open {site} on any device and enter this code:"
            )))
            .build();
        group.add(&shown);
        // The browser may not open (Game Mode): copying the code still works.
        let open = super::action_row(&format!("Open {site}"), "suggested-action");
        open.connect_activated({
            let url = code.verification_url.clone();
            move |_| oauth::open_browser(url.clone())
        });
        let copy = super::action_row("Copy code", "suggested-action");
        copy.connect_activated({
            let user_code = code.user_code.clone();
            move |copy| {
                copy.display().clipboard().set_text(&user_code);
                copy.set_title("Copied");
                glib::timeout_add_seconds_local_once(
                    2,
                    glib::clone!(
                        #[weak]
                        copy,
                        move || copy.set_title("Copy code")
                    ),
                );
            }
        });
        let dialog = super::form_dialog(
            "Log in to Trakt",
            &[&group, &super::actions_group(&[&open, &copy])],
        );
        let cancel = code.cancel.clone();
        dialog.connect_closed({
            let cancel = cancel.clone();
            move |_| cancel.store(true, Ordering::Relaxed)
        });
        dialog.present(Some(&anchor));
        let result = spawn_tokio_checked(async move {
            tracker::get()
                .service(ServiceId::Trakt)
                .login(LoginInput::DeviceCode(code))
                .await
        })
        .await
        .and_then(|result| result);
        let cancelled = cancel.load(Ordering::Relaxed);
        dialog.force_close();
        if cancelled {
            // Closed by the user: nothing to report.
            refresh_row(&row, &suffix, ServiceId::Trakt);
            return;
        }
        finish_login(ServiceId::Trakt, result, &row, &suffix);
    });
}
