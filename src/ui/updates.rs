//! Update checks in the UI: a quiet check at start-up (a notice with an
//! Update button) and the Preferences → Updates group.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::config::Settings;
use crate::runtime::spawn_tokio;
use crate::update::{self, InstallKind, Update};

/// At start-up, for installed builds with the check enabled.
pub fn check_on_startup(toasts: &adw::ToastOverlay) {
    let kind = update::install_kind();
    if kind == InstallKind::Source || !Settings::load().unwrap_or_default().updates.check_on_start {
        return;
    }
    let toasts = toasts.downgrade();
    glib::spawn_future_local(async move {
        let checking = kind.clone();
        let found = spawn_tokio(async move { update::check(&checking).await }).await;
        let (Ok(Some(found)), Some(toasts)) = (found, toasts.upgrade()) else {
            return;
        };
        let toast = adw::Toast::builder()
            .title(format!("EmbyClientPlus {} is available", found.version))
            .button_label("Update")
            .timeout(10)
            .build();
        toast.connect_button_clicked({
            let toasts = toasts.downgrade();
            move |_| {
                if let Some(toasts) = toasts.upgrade() {
                    install(&toasts, kind.clone(), found.clone());
                }
            }
        });
        toasts.add_toast(toast);
    });
}

/// Downloads and installs `found`, then offers a restart.
fn install(toasts: &adw::ToastOverlay, kind: InstallKind, found: Update) {
    toasts.add_toast(notice(&format!("Downloading {}…", found.version)));
    let toasts = toasts.downgrade();
    glib::spawn_future_local(async move {
        let installing = (kind.clone(), found.clone());
        let result =
            spawn_tokio(async move { update::apply(&installing.0, &installing.1).await }).await;
        let Some(toasts) = toasts.upgrade() else {
            return;
        };
        match result {
            Ok(()) => {
                let toast = adw::Toast::builder()
                    .title(format!("Updated to {}", found.version))
                    .button_label("Restart")
                    .timeout(0)
                    .build();
                toast.connect_button_clicked(move |_| restart(&kind));
                toasts.add_toast(toast);
            }
            Err(e) => {
                tracing::warn!("update failed: {e:#}");
                toasts.add_toast(notice(&format!("Update failed: {e}")));
            }
        }
    });
}

fn restart(kind: &InstallKind) {
    if let Err(e) = update::relaunch(kind) {
        tracing::warn!("{e:#}");
        return;
    }
    // Closing the window runs the normal shutdown (playback report, SVP).
    if let Some(window) = gio::Application::default()
        .and_downcast::<gtk::Application>()
        .and_then(|app| app.active_window())
    {
        window.close();
    }
}

fn notice(text: &str) -> adw::Toast {
    adw::Toast::builder()
        .title(glib::markup_escape_text(text))
        .timeout(super::TOAST_SECONDS)
        .build()
}

/// Preferences → Updates.
pub fn preferences_group(toasts: &adw::PreferencesDialog) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Updates").build();
    let kind = update::install_kind();
    let version = adw::ActionRow::builder()
        .title(format!("EmbyClientPlus {}", update::current_version()))
        .build();
    let button = gtk::Button::builder()
        .label("Check for Updates")
        .valign(gtk::Align::Center)
        .build();
    version.add_suffix(&button);
    group.add(&version);

    let on_start = adw::SwitchRow::builder()
        .title("Check at startup")
        .active(Settings::load().unwrap_or_default().updates.check_on_start)
        .build();
    on_start.connect_active_notify(|row| {
        let on = row.is_active();
        if let Err(e) = Settings::update(|s| s.updates.check_on_start = on) {
            tracing::warn!("{e:#}");
        }
    });
    group.add(&on_start);

    if kind == InstallKind::Source {
        version.set_subtitle("Updates are for installed builds (AppImage or install.sh)");
        button.set_sensitive(false);
        on_start.set_sensitive(false);
        return group;
    }
    version.set_subtitle(match &kind {
        InstallKind::AppImage(_) => "AppImage",
        _ => "Installed with install.sh",
    });

    let found: Rc<std::cell::RefCell<Option<Update>>> = Rc::default();
    let dialog = toasts.downgrade();
    button.connect_clicked(move |button| {
        // Second press: install what the first press found.
        if let Some(update) = found.borrow().clone() {
            button.set_sensitive(false);
            button.set_label("Updating…");
            install_from_dialog(&dialog, button, &version, kind.clone(), update);
            return;
        }
        button.set_sensitive(false);
        version.set_subtitle("Checking…");
        let checking = kind.clone();
        let button = button.downgrade();
        let version = version.downgrade();
        let found = found.clone();
        glib::spawn_future_local(async move {
            let result = spawn_tokio(async move { update::check(&checking).await }).await;
            let (Some(button), Some(version)) = (button.upgrade(), version.upgrade()) else {
                return;
            };
            button.set_sensitive(true);
            match result {
                Ok(Some(update)) => {
                    version.set_subtitle(&format!("{} is available", update.version));
                    button.set_label(&format!("Update to {}", update.version));
                    button.add_css_class("suggested-action");
                    found.replace(Some(update));
                }
                Ok(None) => version.set_subtitle("Up to date"),
                Err(e) => {
                    version.set_subtitle(&glib::markup_escape_text(&format!("Couldn't check: {e}")))
                }
            }
        });
    });
    group
}

fn install_from_dialog(
    dialog: &glib::WeakRef<adw::PreferencesDialog>,
    button: &gtk::Button,
    row: &adw::ActionRow,
    kind: InstallKind,
    found: Update,
) {
    let dialog = dialog.clone();
    let button = button.downgrade();
    let row = row.downgrade();
    glib::spawn_future_local(async move {
        let installing = (kind.clone(), found.clone());
        let result =
            spawn_tokio(async move { update::apply(&installing.0, &installing.1).await }).await;
        let (Some(button), Some(row)) = (button.upgrade(), row.upgrade()) else {
            return;
        };
        match result {
            Ok(()) => {
                row.set_subtitle(&format!("Updated to {}; restart to use it", found.version));
                button.set_label("Restart");
                button.set_sensitive(true);
                let kind = kind.clone();
                button.connect_clicked(move |_| restart(&kind));
            }
            Err(e) => {
                tracing::warn!("update failed: {e:#}");
                row.set_subtitle(&glib::markup_escape_text(&format!("Update failed: {e}")));
                button.set_label("Check for Updates");
                button.set_sensitive(true);
                if let Some(dialog) = dialog.upgrade() {
                    dialog.add_toast(notice("Update failed"));
                }
            }
        }
    });
}
