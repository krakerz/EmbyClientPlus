//! App-wide actions: About, Settings and Quit. On macOS GTK puts these in
//! the app's menu in the menu bar (About Emby Client+, Settings… ⌘,,
//! Quit ⌘Q), which stays greyed out without them; elsewhere they give
//! Ctrl+, and Ctrl+Q.

use adw::prelude::*;
use gtk::gio;

/// Adds the actions to `app`. `server`: Settings shows the server's
/// settings (false in the standalone player).
pub fn install(app: &adw::Application, server: bool) {
    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(glib_clone_app(app, |app| {
        let dialog = adw::AboutDialog::builder()
            .application_name(crate::APP_NAME)
            .application_icon(crate::ui::icons::APP)
            .version(crate::update::current_version())
            .website("https://github.com/krakerz/EmbyClientPlus")
            .issue_url("https://github.com/krakerz/EmbyClientPlus/issues")
            .license_type(gtk::License::Gpl30)
            .build();
        dialog.present(app.active_window().as_ref());
    }));
    let preferences = gio::SimpleAction::new("preferences", None);
    preferences.connect_activate(glib_clone_app(app, move |app| {
        if let Some(window) = app.active_window() {
            if server {
                super::preferences::show(&window);
            } else {
                super::preferences::show_player_only(&window);
            }
        }
    }));
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(glib_clone_app(app, |app| match app.active_window() {
        // A clean shutdown: final playback report, SVP Manager stopped.
        Some(window) => super::window::shut_down(window.upcast_ref()),
        None => app.quit(),
    }));
    app.add_action(&about);
    app.add_action(&preferences);
    app.add_action(&quit);
    app.set_accels_for_action("app.preferences", &["<Primary>comma"]);
    app.set_accels_for_action("app.quit", &["<Primary>q"]);
}

/// An action handler holding `app` weakly.
fn glib_clone_app(
    app: &adw::Application,
    run: impl Fn(&adw::Application) + 'static,
) -> impl Fn(&gio::SimpleAction, Option<&gtk::glib::Variant>) + 'static {
    let weak = app.downgrade();
    move |_, _| {
        if let Some(app) = weak.upgrade() {
            run(&app);
        }
    }
}
