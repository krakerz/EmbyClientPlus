//! Server address + credentials form.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::emby::EmbyClient;
use crate::runtime::spawn_tokio;

/// What a successful login hands back to the window.
pub struct LoggedIn {
    pub client: EmbyClient,
    pub server_url: String,
    pub user_id: String,
    pub username: String,
    pub token: String,
}

#[derive(Clone)]
pub struct LoginPage {
    root: gtk::Widget,
    server: adw::EntryRow,
    username: adw::EntryRow,
    password: adw::PasswordEntryRow,
    banner: adw::Banner,
}

impl LoginPage {
    pub fn new(device_id: String, on_login: impl Fn(LoggedIn) + 'static) -> Self {
        let server = adw::EntryRow::builder().title("Server address").build();
        let username = adw::EntryRow::builder().title("Username").build();
        let password = adw::PasswordEntryRow::builder().title("Password").build();
        let group = adw::PreferencesGroup::new();
        group.add(&server);
        group.add(&username);
        group.add(&password);

        let connect = gtk::Button::builder()
            .label("Connect")
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        let spinner = adw::Spinner::builder().visible(false).build();
        let actions = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        actions.append(&connect);
        actions.append(&spinner);

        let form = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .build();
        form.append(&group);
        form.append(&actions);
        let status = adw::StatusPage::builder()
            .icon_name(crate::ui::icons::DISPLAY)
            .title("EmbyClientPlus")
            .description("Sign in to your Emby server")
            .child(&adw::Clamp::builder().maximum_size(420).child(&form).build())
            .vexpand(true)
            .build();

        let banner = adw::Banner::new("");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&banner);
        content.append(&status);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
        toolbar.set_content(Some(&content));

        let page = LoginPage {
            root: toolbar.upcast(),
            server,
            username,
            password,
            banner,
        };

        let on_login = Rc::new(on_login);
        let submit = {
            let page = page.clone();
            let connect = connect.clone();
            move || {
                let Some(url) = normalize_server_url(&page.server.text()) else {
                    page.show_error("Enter the server address, e.g. 192.168.1.3:8096");
                    return;
                };
                let username = page.username.text().trim().to_string();
                if username.is_empty() {
                    page.show_error("Enter your username");
                    return;
                }
                let password = page.password.text().to_string();
                page.banner.set_revealed(false);
                connect.set_sensitive(false);
                spinner.set_visible(true);

                let page = page.clone();
                let connect = connect.clone();
                let spinner = spinner.clone();
                let on_login = on_login.clone();
                let device_id = device_id.clone();
                glib::spawn_future_local(async move {
                    let result = spawn_tokio({
                        let url = url.clone();
                        let username = username.clone();
                        async move {
                            let mut client = EmbyClient::new(url, device_id);
                            let response =
                                client.authenticate_by_name(&username, &password).await?;
                            Ok::<_, anyhow::Error>((client, response))
                        }
                    })
                    .await;
                    connect.set_sensitive(true);
                    spinner.set_visible(false);
                    match result {
                        Ok((client, response)) => {
                            page.password.set_text("");
                            on_login(LoggedIn {
                                client,
                                server_url: url,
                                user_id: response.user.id,
                                username: response.user.name,
                                token: response.access_token,
                            });
                        }
                        Err(e) => {
                            tracing::warn!("login failed: {e:#}");
                            let message = if crate::emby::is_unauthorized(&e) {
                                "Wrong username or password".to_string()
                            } else {
                                format!("Could not reach the server: {e}")
                            };
                            page.show_error(&message);
                        }
                    }
                });
            }
        };
        let submit = Rc::new(submit);
        connect.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        page.password.connect_entry_activated(move |_| submit());
        page
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.root
    }

    pub fn prefill(&self, server_url: &str, username: &str) {
        self.server.set_text(server_url);
        self.username.set_text(username);
    }

    pub fn show_error(&self, message: &str) {
        self.banner.set_title(message);
        self.banner.set_revealed(true);
    }
}

/// Turns what the user typed into the base URL the client prefixes `/emby`
/// paths with: adds a scheme, drops trailing slashes and a trailing `/emby`.
pub fn normalize_server_url(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let base = with_scheme
        .strip_suffix("/emby")
        .unwrap_or(&with_scheme)
        .trim_end_matches('/');
    Some(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_server_url;

    #[test]
    fn bare_host_gets_http_scheme() {
        assert_eq!(
            normalize_server_url(" 192.168.1.3:8096/ ").as_deref(),
            Some("http://192.168.1.3:8096")
        );
    }

    #[test]
    fn existing_scheme_and_emby_suffix() {
        assert_eq!(
            normalize_server_url("https://media.example.com/emby/").as_deref(),
            Some("https://media.example.com")
        );
    }

    #[test]
    fn empty_input_is_rejected() {
        assert_eq!(normalize_server_url("   "), None);
    }
}
