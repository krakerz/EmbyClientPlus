//! Top-level window: switches between the login form and a logged-in `Ui`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, glib};

use super::login::{LoggedIn, LoginPage};
use super::player_page::PlayerPage;
use super::{Session, Ui, images};
use crate::auth;
use crate::config::Settings;
use crate::emby::EmbyClient;
use crate::player::Player;
use crate::runtime::spawn_detached;

const CSS: &str = "
.card-button { padding: 0; }
.player-bar {
    margin: 12px;
    padding: 8px 14px;
    border-radius: 14px;
}
.scrub-preview {
    padding: 8px;
    border-radius: 12px;
}
.scrub-frame { border-radius: 8px; }
.legend {
    padding: 6px 14px;
    border-radius: 999px;
    font-size: 0.9em;
    background: @popover_bg_color;
    color: @popover_fg_color;
    border: 1px solid alpha(@borders, 0.8);
    box-shadow: 0 2px 8px alpha(black, 0.35);
}
.queue-moving {
    background: alpha(@accent_bg_color, 0.35);
    outline: 3px solid @accent_color;
    outline-offset: -3px;
}
.legend-key {
    min-width: 22px;
    padding: 0 6px;
    border-radius: 6px;
    background: alpha(currentColor, 0.18);
    font-weight: bold;
    font-size: 0.85em;
}
.volume-osd {
    padding: 12px 18px;
    border-radius: 999px;
}
.up-next {
    padding: 12px;
    border-radius: 14px;
}
.skip-button {
    padding: 10px 22px;
    font-weight: bold;
}
.large-button {
    min-width: 48px;
    min-height: 48px;
}
.svp-active { color: @success_color; }
.svp-waiting { color: @warning_color; }
.svp-missing { color: @error_color; }
.person-card { border-radius: 999px; }
.art-placeholder { opacity: 0.45; }
.category-tile { border-radius: 14px; }
.tile-shade {
    background: linear-gradient(to top, rgba(0, 0, 0, 0.85), rgba(0, 0, 0, 0) 70%);
}
.tile-name {
    color: white;
    font-weight: 800;
    font-size: 1.35em;
}
.tile-count { color: rgba(255, 255, 255, 0.75); }
flowboxchild:focus-visible .category-tile { outline: 3px solid @accent_color; outline-offset: 2px; }
/* Controller/keyboard focus: a solid accent ring, easy to spot from the couch. */
button:focus-visible { outline: 3px solid @accent_color; outline-offset: 1px; }
.card-button:focus-visible { outline: none; }
/* Grid cells draw their own focus box around the whole card; the ring on
   the artwork (below) is the one to show. */
gridview > child:focus-visible {
    outline: none;
    background: none;
}
.card-button:focus-visible .card,
gridview > child:focus-visible .card {
    outline: 4px solid @accent_color;
    outline-offset: -4px;
}
.card-button:focus-visible .card-title,
gridview > child:focus-visible .card-title { color: @accent_color; }
row:focus-visible { outline: 3px solid @accent_color; outline-offset: -3px; }
/* Menus (player options, card menus): the highlighted choice stands out
   from video behind the popover, not just a faint tint. */
popover.menu modelbutton:hover,
popover.menu modelbutton:focus-visible,
popover modelbutton:focus-visible {
    background: @accent_bg_color;
    color: @accent_fg_color;
}
.watched-badge {
    background: alpha(@accent_bg_color, 0.9);
    color: @accent_fg_color;
    border-radius: 999px;
    padding: 3px;
}
";

struct State {
    stack: gtk::Stack,
    toasts: adw::ToastOverlay,
    login: RefCell<Option<LoginPage>>,
    ui: RefCell<Option<Ui>>,
    player: Player,
    player_page: PlayerPage,
    legend: super::legend::Legend,
    window: glib::WeakRef<adw::ApplicationWindow>,
}

impl State {
    /// Re-reads the controller legend for what's on screen.
    fn refresh_legend(&self) {
        if let Some(window) = self.window.upgrade() {
            self.legend.update(&window, self.ui.borrow().as_ref());
        }
    }
}

/// Shows the window. SVP Manager starts with the app (when auto-start is
/// on), so it's up before the first video. In Game Mode, gamescope gives
/// focus to whichever window maps last, so if SVP Manager has to start,
/// ours waits until it's up and then opens on top.
pub fn present(window: &adw::ApplicationWindow) {
    let frame_gen = Settings::load().unwrap_or_default().frame_gen;
    let wanted = frame_gen.default_backend == crate::config::FrameGenBackend::Svp
        && frame_gen.auto_start_svp()
        && crate::svp::installed();
    if !wanted || crate::svp::manager_running() {
        window.present();
        return;
    }
    crate::svp::ensure_running();
    if !crate::gamescope::detected() {
        window.present();
        return;
    }
    let started = std::time::Instant::now();
    let running_since = std::cell::Cell::new(None::<std::time::Instant>);
    let window = window.downgrade();
    glib::timeout_add_local(SVP_POLL, move || {
        let Some(window) = window.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if running_since.get().is_none() && crate::svp::manager_running() {
            running_since.set(Some(std::time::Instant::now()));
        }
        let settled = running_since
            .get()
            .is_some_and(|at| at.elapsed() >= SVP_SETTLE);
        if settled || started.elapsed() >= SVP_WAIT_MAX {
            window.present();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

thread_local! {
    /// The window a termination signal closes (main thread only).
    static SHUTDOWN_WINDOW: RefCell<glib::WeakRef<adw::ApplicationWindow>> =
        RefCell::new(glib::WeakRef::new());
}

/// Quits the app the normal way (final playback report, SVP Manager
/// stopped). An open dialog would swallow the close request (libadwaita
/// closes the dialog instead), so it goes first.
pub fn shut_down(window: &gtk::Window) {
    if let Some(dialog) = window
        .downcast_ref::<adw::ApplicationWindow>()
        .and_then(|window| window.visible_dialog())
    {
        dialog.force_close();
    }
    window.close();
}

/// After a termination signal, the longest a clean shutdown may take.
const SHUTDOWN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolves when the first of `futures` does.
async fn futures_select_any<F: std::future::Future + Unpin>(futures: impl Iterator<Item = F>) {
    let mut futures: Vec<F> = futures.collect();
    std::future::poll_fn(move |cx| {
        for future in &mut futures {
            if std::pin::Pin::new(future).poll(cx).is_ready() {
                return std::task::Poll::Ready(());
            }
        }
        std::task::Poll::Pending
    })
    .await;
}

/// Pointer movement (px) below this is noise, not someone using the mouse.
const POINTER_JITTER: f64 = 4.0;

fn pointer_used(state: &State) {
    if state.legend.is_in_use() {
        state.legend.set_in_use(false);
        state.refresh_legend();
    }
}

const SVP_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// How long SVP Manager gets to open its window after its process appears.
const SVP_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);
/// Never keep the app hidden longer than this.
const SVP_WAIT_MAX: std::time::Duration = std::time::Duration::from_secs(10);

pub fn build(app: &adw::Application, player: Player) -> adw::ApplicationWindow {
    load_css();
    images::prune_disk_cache();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    let toasts = adw::ToastOverlay::new();
    let legend = super::legend::Legend::new();
    let content = gtk::Overlay::builder().child(&stack).build();
    content.add_overlay(legend.widget());
    toasts.set_child(Some(&content));
    let window_settings = Settings::load().unwrap_or_default().window;
    apply_theme(window_settings.theme);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(crate::APP_NAME)
        .default_width(window_settings.width.max(640))
        .default_height(window_settings.height.max(400))
        .content(&toasts)
        .build();

    // The window gets a real size first (above), so a cold-launch
    // fullscreen in gamescope doesn't fall back to 800×600.
    if window_settings.start_fullscreen(crate::gamescope::detected()) {
        window.connect_map(|window| window.fullscreen());
    }

    let settings = Settings::update(|settings| {
        settings.server.ensure_device_id();
    })
    .unwrap_or_else(|e| {
        tracing::warn!("settings unreadable, using defaults: {e:#}");
        let mut settings = Settings::default();
        settings.server.ensure_device_id();
        settings
    });

    let state = Rc::new(State {
        stack,
        toasts,
        login: RefCell::new(None),
        ui: RefCell::new(None),
        player,
        player_page: PlayerPage::new(player),
        legend,
        window: window.downgrade(),
    });

    match stored_session(&settings) {
        Some(session) => show_main(&state, session),
        None => show_login(&state, None),
    }

    super::updates::check_on_startup(&state.toasts);

    // Controller presses go to whatever is on screen.
    crate::controller::start({
        let state = Rc::downgrade(&state);
        let window = window.downgrade();
        move |pad, repeat| {
            if let (Some(state), Some(window)) = (state.upgrade(), window.upgrade()) {
                let ui = state.ui.borrow().clone();
                super::gamepad::handle(&window, ui.as_ref(), pad, repeat);
                state.legend.set_in_use(true);
                state.refresh_legend();
            }
        }
    });

    super::gamepad::track_focus(&window);
    // Steam's "Exit game", a logout or Ctrl+C: shut down as if the window
    // were closed (final playback report, SVP Manager stopped).
    SHUTDOWN_WINDOW.with(|slot| slot.replace(window.downgrade()));
    spawn_detached(async {
        use tokio::signal::unix::{SignalKind, signal};
        let kinds = [
            SignalKind::terminate(),
            SignalKind::interrupt(),
            SignalKind::hangup(),
        ];
        let mut streams: Vec<_> = kinds.into_iter().filter_map(|k| signal(k).ok()).collect();
        if streams.is_empty() {
            return;
        }
        let waits = streams.iter_mut().map(|s| Box::pin(s.recv()));
        futures_select_any(waits).await;
        tracing::info!("termination signal: shutting down");
        glib::MainContext::default().invoke(|| {
            if let Some(window) = SHUTDOWN_WINDOW.with(|slot| slot.borrow().upgrade()) {
                shut_down(window.upcast_ref());
            }
        });
        // The signal no longer kills us by default: if shutdown hangs,
        // don't outlive the request.
        tokio::time::sleep(SHUTDOWN_DEADLINE).await;
        tracing::warn!("shutdown took too long; exiting");
        std::process::exit(0);
    });
    // The legend follows the controller, the mapping and open dialogs.
    crate::controller::on_change({
        let state = Rc::downgrade(&state);
        move || {
            if let Some(state) = state.upgrade() {
                state.refresh_legend();
            }
        }
    });
    // Mouse or touch input hides the legend until the controller is used
    // again. Motion events also come without the pointer moving (layout
    // changes), so only real movement counts.
    let pointer_moved = gtk::EventControllerMotion::new();
    pointer_moved.set_propagation_phase(gtk::PropagationPhase::Capture);
    let last_position = std::cell::Cell::new(None::<(f64, f64)>);
    pointer_moved.connect_motion({
        let state = Rc::downgrade(&state);
        move |_, x, y| {
            let moved = last_position
                .replace(Some((x, y)))
                .is_some_and(|(px, py)| (x - px).abs() + (y - py).abs() > POINTER_JITTER);
            if moved && let Some(state) = state.upgrade() {
                pointer_used(&state);
            }
        }
    });
    window.add_controller(pointer_moved);
    let clicked = gtk::GestureClick::new();
    clicked.set_button(0);
    clicked.set_propagation_phase(gtk::PropagationPhase::Capture);
    clicked.connect_pressed({
        let state = Rc::downgrade(&state);
        move |_, _, _, _| {
            if let Some(state) = state.upgrade() {
                pointer_used(&state);
            }
        }
    });
    window.add_controller(clicked);
    window.connect_visible_dialog_notify({
        let state = Rc::downgrade(&state);
        move |_| {
            if let Some(state) = state.upgrade() {
                state.refresh_legend();
            }
        }
    });
    window.connect_close_request({
        let state = state.clone();
        move |_| {
            if let Some(ui) = state.ui.borrow().as_ref() {
                ui.stop_playback_and_wait();
            }
            if let Err(e) = player.quit() {
                tracing::warn!("{e:#}");
            }
            crate::svp::stop_started();
            glib::Propagation::Proceed
        }
    });
    window
}

/// The session saved by the last login, if its token is still stored.
fn stored_session(settings: &Settings) -> Option<Session> {
    let server = &settings.server;
    if server.url.is_empty() || server.user_id.is_empty() {
        return None;
    }
    let token = match auth::get_token(&server.url) {
        Ok(Some(token)) => token,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("could not read the stored token: {e:#}");
            return None;
        }
    };
    let client = EmbyClient::new(&server.url, &server.device_id).with_token(token);
    Some(Session {
        client: Arc::new(client),
        user_id: server.user_id.clone(),
    })
}

fn show_main(state: &Rc<State>, session: Session) {
    let weak = Rc::downgrade(state);
    let ui = Ui::new(
        session,
        state.toasts.clone(),
        state.player,
        state.player_page.clone(),
        move |expired| {
            if let Some(state) = weak.upgrade() {
                // Defer: this runs inside a callback of the Ui being torn down.
                glib::idle_add_local_once(move || logout(&state, expired));
            }
        },
    );
    replace_child(&state.stack, "main", ui.widget());
    state.stack.set_visible_child_name("main");
    // Pages (and the player) come and go: the legend's hints change too.
    ui.nav().connect_visible_page_notify({
        let state = Rc::downgrade(state);
        move |_| {
            if let Some(state) = state.upgrade() {
                state.refresh_legend();
            }
        }
    });
    // ...and so do the music panel opening and the music bar appearing.
    let sheet = ui.widget();
    for property in ["open", "bottom-bar"] {
        let state = Rc::downgrade(state);
        sheet.connect_notify_local(Some(property), move |_, _| {
            if let Some(state) = state.upgrade() {
                state.refresh_legend();
            }
        });
    }
    state.ui.replace(Some(ui));
    state.refresh_legend();
    state.login.replace(None);
    if let Some(child) = state.stack.child_by_name("login") {
        state.stack.remove(&child);
    }
}

fn show_login(state: &Rc<State>, error: Option<&str>) {
    let settings = Settings::load().unwrap_or_default();
    let device_id = settings.server.device_id.clone();
    let weak = Rc::downgrade(state);
    let login = LoginPage::new(device_id, move |logged_in| {
        if let Some(state) = weak.upgrade() {
            on_logged_in(&state, logged_in);
        }
    });
    login.prefill(&settings.server.url, &settings.server.username);
    if let Some(error) = error {
        login.show_error(error);
    }
    replace_child(&state.stack, "login", login.widget());
    state.stack.set_visible_child_name("login");
    state.login.replace(Some(login));
}

fn on_logged_in(state: &Rc<State>, logged_in: LoggedIn) {
    if let Err(e) = auth::store_token(&logged_in.server_url, &logged_in.token) {
        tracing::warn!("could not store the token, next launch will ask again: {e:#}");
    }
    let user_id = logged_in.user_id.clone();
    if let Err(e) = Settings::update(|settings| {
        settings.server.url = logged_in.server_url;
        settings.server.user_id = user_id;
        settings.server.username = logged_in.username;
    }) {
        tracing::warn!("{e:#}");
    }
    show_main(
        state,
        Session {
            client: Arc::new(logged_in.client),
            user_id: logged_in.user_id,
        },
    );
}

/// Back to the login form. `expired` means the server rejected the token,
/// so there's no server session left to end.
fn logout(state: &Rc<State>, expired: bool) {
    let Some(ui) = state.ui.take() else { return };
    ui.stop_playback();
    if !expired {
        let client = ui.client();
        spawn_detached(async move {
            if let Err(e) = client.logout().await {
                tracing::debug!("server logout failed: {e:#}");
            }
        });
    }
    let url = Settings::load().unwrap_or_default().server.url;
    if let Err(e) = auth::delete_token(&url) {
        tracing::warn!("{e:#}");
    }
    if let Err(e) = Settings::update(|settings| settings.server.user_id.clear()) {
        tracing::warn!("{e:#}");
    }
    images::clear_cache();
    show_login(
        state,
        expired.then_some("Your session expired, please sign in again"),
    );
    if let Some(child) = state.stack.child_by_name("main") {
        state.stack.remove(&child);
    }
}

/// Whether the app keeps the whole window fullscreen (gamescope with the
/// default "auto" setting, or "always"), so the player never shrinks it.
pub fn fullscreen_locked() -> bool {
    crate::gamescope::detected()
        && Settings::load()
            .unwrap_or_default()
            .window
            .start_fullscreen(true)
}

fn replace_child(stack: &gtk::Stack, name: &str, child: &impl IsA<gtk::Widget>) {
    if let Some(old) = stack.child_by_name(name) {
        stack.remove(&old);
    }
    stack.add_named(child, Some(name));
}

/// Applies the colour scheme from Preferences.
pub fn apply_theme(theme: crate::config::Theme) {
    let scheme = match theme {
        crate::config::Theme::System => adw::ColorScheme::Default,
        crate::config::Theme::Light => adw::ColorScheme::ForceLight,
        crate::config::Theme::Dark => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
