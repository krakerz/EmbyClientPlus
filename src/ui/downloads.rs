//! Downloads: starting them (item menus), the Downloads page (running,
//! waiting and finished ones), and playing them from disk. One download
//! runs at a time; the rest wait in a queue (a whole season or series).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use gtk::glib;

use super::{Ui, clear, fixed_picture, format_runtime};
use crate::downloads::Download;
use crate::emby::models::BaseItem;
use crate::update::Progress;

/// A download under way.
#[derive(Clone)]
struct Running {
    item: BaseItem,
    progress: Arc<Progress>,
    cancelled: Arc<AtomicBool>,
}

thread_local! {
    static RUNNING: RefCell<Vec<Running>> = const { RefCell::new(Vec::new()) };
    static QUEUE: RefCell<VecDeque<BaseItem>> = const { RefCell::new(VecDeque::new()) };
    /// An auto-download check is under way.
    static SYNCING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The check waiting to run after playback stops.
    static PENDING_SYNC: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

/// Movies and episodes can be downloaded.
pub fn can_download(item: &BaseItem) -> bool {
    matches!(item.item_type.as_str(), "Movie" | "Episode" | "Video")
}

/// Whether `item_id` is downloading or waiting to.
pub fn is_running(item_id: &str) -> bool {
    RUNNING.with(|r| r.borrow().iter().any(|d| d.item.id == item_id))
        || QUEUE.with(|q| q.borrow().iter().any(|i| i.id == item_id))
}

/// Downloads `item` (once), after whatever is already downloading.
pub fn start(ui: &Ui, item: &BaseItem) {
    if enqueue(item) {
        if RUNNING.with(|r| r.borrow().is_empty()) {
            ui.toast(&format!("Downloading {}", item.episode_label()));
        } else {
            ui.toast(&format!("{} will download next", item.episode_label()));
        }
        pump(ui);
    }
}

/// Queues `item` unless it's downloaded, downloading or queued already.
fn enqueue(item: &BaseItem) -> bool {
    if is_running(&item.id) || crate::downloads::find(&item.id).is_some() {
        return false;
    }
    QUEUE.with(|q| q.borrow_mut().push_back(item.clone()));
    true
}

/// Downloads every episode of `target` (a series or a season) not yet
/// downloaded, in order.
pub fn start_episodes_of(ui: &Ui, target: &BaseItem) {
    let series_id = match target.item_type.as_str() {
        "Series" => target.id.clone(),
        _ => match &target.series_id {
            Some(id) => id.clone(),
            None => return,
        },
    };
    let season = (target.item_type == "Season").then(|| target.id.clone());
    let (client, user_id) = (ui.client(), ui.user_id());
    let weak = ui.downgrade();
    let name = target.name.clone();
    glib::spawn_future_local(async move {
        let episodes = crate::runtime::spawn_tokio(async move {
            client.series_episodes(&series_id, &user_id).await
        })
        .await;
        let Some(ui) = weak.upgrade() else { return };
        let episodes = match episodes {
            Ok(episodes) => episodes,
            Err(e) => return ui.report_error(&format!("Couldn't list the episodes of {name}"), &e),
        };
        let added = episodes
            .iter()
            .filter(|e| season.is_none() || e.season_id == season)
            .filter(|e| enqueue(e))
            .count();
        match added {
            0 => ui.toast(&format!("Everything in {name} is downloaded already")),
            1 => ui.toast(&format!("Downloading 1 episode of {name}")),
            n => ui.toast(&format!("Downloading {n} episodes of {name}")),
        }
        pump(&ui);
        ui.data_changed();
    });
}

/// Starts the next queued download if none is running.
fn pump(ui: &Ui) {
    if !RUNNING.with(|r| r.borrow().is_empty()) {
        return;
    }
    if let Some(item) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
        run(ui, &item);
    }
}

fn run(ui: &Ui, item: &BaseItem) {
    let running = Running {
        item: item.clone(),
        progress: Arc::new(Progress::default()),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    RUNNING.with(|r| r.borrow_mut().push(running.clone()));
    let (client, user_id) = (ui.client(), ui.user_id());
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let (progress, cancelled, id) = (
            running.progress.clone(),
            running.cancelled.clone(),
            running.item.id.clone(),
        );
        let result = crate::runtime::spawn_tokio(crate::downloads::fetch(
            client, user_id, id, progress, cancelled,
        ))
        .await;
        RUNNING.with(|r| r.borrow_mut().retain(|d| d.item.id != running.item.id));
        let Some(ui) = weak.upgrade() else { return };
        let name = running.item.episode_label();
        match result {
            Ok(_) => ui.toast(&format!("Downloaded {name}")),
            Err(_) if running.cancelled.load(Ordering::Relaxed) => {}
            Err(e) => ui.report_error(&format!("Couldn't download {name}"), &e),
        }
        pump(&ui);
        ui.data_changed();
    });
}

/// Deletes `item`'s download (from a menu).
pub fn remove(ui: &Ui, item: &BaseItem) {
    let Some(download) = crate::downloads::find(&item.id) else {
        return;
    };
    match crate::downloads::delete(&download) {
        Ok(()) => ui.toast(&format!("Deleted the download of {}", item.episode_label())),
        Err(e) => ui.report_error("Couldn't delete the download", &e),
    }
    ui.data_changed();
}

pub fn page(ui: &Ui) -> adw::NavigationPage {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(1000)
        .child(&content)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .vexpand(true)
        .build();
    let header = adw::HeaderBar::new();
    super::add_home_button(&header);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));
    let page = adw::NavigationPage::new(&toolbar, "Downloads");

    let fill = Rc::new({
        let (weak, content) = (ui.downgrade(), content.downgrade());
        move || {
            if let (Some(ui), Some(content)) = (weak.upgrade(), content.upgrade()) {
                fill(&ui, &content);
            }
        }
    });
    fill();
    super::reload_on_change(ui, &page, {
        let fill = fill.clone();
        move || fill()
    });
    // Running downloads' progress, while the page is up.
    let ticker = glib::timeout_add_local(std::time::Duration::from_millis(500), {
        let (content, fill) = (content.downgrade(), fill.clone());
        let shown = RefCell::new(running_ids());
        move || {
            let Some(content) = content.upgrade() else {
                return glib::ControlFlow::Break;
            };
            // A download started or finished: rebuild; else just progress.
            let now = running_ids();
            if *shown.borrow() != now {
                shown.replace(now);
                fill();
            } else {
                update_progress(&content);
            }
            glib::ControlFlow::Continue
        }
    });
    let ticker = RefCell::new(Some(ticker));
    page.connect_hidden(move |_| {
        if let Some(source) = ticker.take() {
            source.remove();
        }
    });
    page
}

/// What's downloading and waiting: the page rebuilds when this changes.
fn running_ids() -> Vec<String> {
    RUNNING
        .with(|r| {
            r.borrow()
                .iter()
                .map(|d| d.item.id.clone())
                .collect::<Vec<_>>()
        })
        .into_iter()
        .chain(QUEUE.with(|q| q.borrow().iter().map(|i| i.id.clone()).collect::<Vec<_>>()))
        .collect()
}

const PROGRESS_KEY: &str = "embyclientplus-download-progress";

fn fill(ui: &Ui, content: &gtk::Box) {
    let mark = super::gamepad::mark_cursor(content);
    clear(content);
    let running: Vec<Running> = RUNNING.with(|r| r.borrow().clone());
    let waiting: Vec<BaseItem> = QUEUE.with(|q| q.borrow().iter().cloned().collect());
    let finished = crate::downloads::list();
    let folder = crate::downloads::folder();
    content.append(
        &gtk::Label::builder()
            .label(format!("Saved in {}", folder.display()))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    let jobs = crate::auto_downloads::for_user(&ui.user_id());
    if running.is_empty() && waiting.is_empty() && finished.is_empty() && jobs.is_empty() {
        content.append(
            &adw::StatusPage::builder()
                .icon_name(crate::ui::icons::DOWNLOAD)
                .title("No downloads")
                .description(
                    "Choose Download in a movie's, episode's, season's or series' menu to watch \
                     it offline, or Auto-download in a series' to keep its next episodes here",
                )
                .vexpand(true)
                .build(),
        );
        return;
    }
    if !jobs.is_empty() {
        let list = section(content, "Auto-download");
        for job in &jobs {
            list.append(&job_row(ui, job));
        }
    }
    if !running.is_empty() {
        let list = section(content, "Downloading");
        for download in running {
            list.append(&running_row(&download));
        }
    }
    if !waiting.is_empty() {
        let list = section(content, "Waiting");
        for item in waiting {
            list.append(&waiting_row(ui, &item));
        }
    }
    if !finished.is_empty() {
        let list = section(content, "Downloaded");
        for download in finished {
            list.append(&finished_row(ui, &download));
        }
    }
    if let Some(mark) = mark {
        mark.restore();
    }
}

fn section(content: &gtk::Box, title: &str) -> gtk::ListBox {
    content.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    content.append(&list);
    list
}

fn title_of(item: &BaseItem) -> String {
    match (&item.series_name, item.item_type.as_str()) {
        (Some(series), "Episode") => format!("{series} · {}", item.episode_label()),
        _ => item.name.clone(),
    }
}

fn running_row(download: &Running) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&title_of(&download.item)))
        .subtitle(download.progress.describe())
        .build();
    let bar = gtk::ProgressBar::builder()
        .valign(gtk::Align::Center)
        .width_request(160)
        .build();
    if let Some(fraction) = download.progress.fraction() {
        bar.set_fraction(fraction);
    }
    row.add_suffix(&bar);
    let cancel = gtk::Button::builder()
        .icon_name(crate::ui::icons::REMOVE)
        .tooltip_text("Cancel")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let cancelled = download.cancelled.clone();
    cancel.connect_clicked(move |_| cancelled.store(true, Ordering::Relaxed));
    row.add_suffix(&cancel);
    // SAFETY: only ever stored and read as this type.
    unsafe {
        row.set_data(PROGRESS_KEY, (download.progress.clone(), bar));
    }
    row
}

fn waiting_row(ui: &Ui, item: &BaseItem) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&title_of(item)))
        .build();
    let cancel = gtk::Button::builder()
        .icon_name(crate::ui::icons::REMOVE)
        .tooltip_text("Don't download")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let (weak, id) = (ui.downgrade(), item.id.clone());
    cancel.connect_clicked(move |_| {
        QUEUE.with(|q| q.borrow_mut().retain(|i| i.id != id));
        if let Some(ui) = weak.upgrade() {
            ui.data_changed();
        }
    });
    row.add_suffix(&cancel);
    row
}

/// Refreshes running rows' bars and sizes in place.
fn update_progress(content: &gtk::Box) {
    let mut stack = vec![content.clone().upcast::<gtk::Widget>()];
    while let Some(widget) = stack.pop() {
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            stack.push(current);
        }
        let Some(row) = widget.downcast_ref::<adw::ActionRow>() else {
            continue;
        };
        // SAFETY: see `running_row`.
        let Some(data) = (unsafe { row.data::<(Arc<Progress>, gtk::ProgressBar)>(PROGRESS_KEY) })
        else {
            continue;
        };
        // SAFETY: the data lives as long as the row, which we hold.
        let (progress, bar) = unsafe { data.as_ref() };
        row.set_subtitle(&progress.describe());
        if let Some(fraction) = progress.fraction() {
            bar.set_fraction(fraction);
        } else {
            bar.pulse();
        }
    }
}

fn finished_row(ui: &Ui, download: &Download) -> adw::ActionRow {
    let item = &download.item;
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&title_of(item)))
        .activatable(true)
        .build();
    let mut details = Vec::new();
    if let Some(ticks) = item.run_time_ticks {
        details.push(format_runtime(ticks));
    }
    details.push(super::format_size(download.size()));
    row.set_subtitle(&glib::markup_escape_text(&details.join(" · ")));
    let thumb = match &download.image {
        Some(path) => gtk::Picture::for_filename(path),
        None => gtk::Picture::new(),
    };
    thumb.set_content_fit(gtk::ContentFit::Cover);
    let frame = gtk::Frame::builder()
        .child(&fixed_picture(&thumb, 160, 90))
        .valign(gtk::Align::Center)
        .margin_top(6)
        .margin_bottom(6)
        .css_classes(["card"])
        .build();
    row.add_prefix(&frame);
    let delete = gtk::Button::builder()
        .icon_name(crate::ui::icons::REMOVE)
        .tooltip_text("Delete the download")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let (weak, target) = (ui.downgrade(), item.clone());
    delete.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            remove(&ui, &target);
        }
    });
    row.add_suffix(&delete);
    let (weak, target) = (ui.downgrade(), download.clone());
    row.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.play_download(&target, target.item.resume_ticks());
        }
    });
    row
}

/// An auto-download job on the Downloads page: edit or stop it.
fn job_row(ui: &Ui, job: &crate::auto_downloads::Job) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&job.series_name))
        .subtitle(job_summary(job))
        .activatable(true)
        .build();
    let stop = gtk::Button::builder()
        .icon_name(crate::ui::icons::REMOVE)
        .tooltip_text("Stop auto-downloading (keeps the downloads)")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let (weak, user_id, series_id) = (ui.downgrade(), job.user_id.clone(), job.series_id.clone());
    stop.connect_clicked(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        if let Err(e) = crate::auto_downloads::remove(&user_id, &series_id) {
            ui.report_error("Couldn't stop auto-downloading", &e);
        }
        ui.data_changed();
    });
    row.add_suffix(&stop);
    let series = BaseItem {
        id: job.series_id.clone(),
        name: job.series_name.clone(),
        item_type: "Series".into(),
        ..Default::default()
    };
    let weak = ui.downgrade();
    row.connect_activated(move |row| {
        if let Some(ui) = weak.upgrade() {
            auto_dialog(&ui, &series, row);
        }
    });
    row
}

fn job_summary(job: &crate::auto_downloads::Job) -> String {
    let keep = match job.keep {
        1 => "Keeps the next unwatched episode".to_string(),
        n => format!("Keeps the next {n} unwatched episodes"),
    };
    if job.delete_watched {
        format!("{keep}; deletes watched ones")
    } else {
        keep
    }
}

/// Auto-download settings for a series (or the series of a season or
/// episode): how many unwatched episodes to keep, and whether watched ones
/// are deleted.
pub fn auto_dialog(ui: &Ui, item: &BaseItem, parent: &impl IsA<gtk::Widget>) {
    let series = match item.item_type.as_str() {
        "Series" => item.clone(),
        _ => match (&item.series_id, &item.series_name) {
            (Some(id), Some(name)) => BaseItem {
                id: id.clone(),
                name: name.clone(),
                item_type: "Series".into(),
                ..Default::default()
            },
            _ => return,
        },
    };
    let user_id = ui.user_id();
    let existing = crate::auto_downloads::get(&user_id, &series.id);
    let keep = adw::SpinRow::builder()
        .title("Unwatched episodes to keep")
        .adjustment(&gtk::Adjustment::new(
            f64::from(
                existing
                    .as_ref()
                    .map_or(crate::auto_downloads::DEFAULT_KEEP, |j| j.keep),
            ),
            1.0,
            f64::from(crate::auto_downloads::MAX_KEEP),
            1.0,
            5.0,
            0.0,
        ))
        .build();
    let delete_watched = adw::SwitchRow::builder()
        .title("Delete watched episodes")
        .active(existing.as_ref().is_none_or(|j| j.delete_watched))
        .build();
    let settings = adw::PreferencesGroup::builder()
        .title(glib::markup_escape_text(&series.name))
        .description(
            "Keeps its next unwatched episodes downloaded (original files), checking when the \
             app starts, every half hour and after you watch something",
        )
        .build();
    settings.add(&keep);
    settings.add(&delete_watched);
    let save = super::action_row(
        if existing.is_some() { "Save" } else { "Start" },
        "suggested-action",
    );
    let actions = super::actions_group(&[&save]);
    let stop = existing.is_some().then(|| {
        let stop = super::action_row("Stop Auto-downloading", "destructive-action");
        actions.add(&stop);
        stop
    });
    let dialog = super::form_dialog("Auto-download", &[&settings, &actions]);

    let weak = ui.downgrade();
    let (dialog_ref, series_ref, user_ref) = (dialog.downgrade(), series.clone(), user_id.clone());
    save.connect_activated(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let job = crate::auto_downloads::Job {
            user_id: user_ref.clone(),
            series_id: series_ref.id.clone(),
            series_name: series_ref.name.clone(),
            keep: keep.value() as u32,
            delete_watched: delete_watched.is_active(),
        };
        match crate::auto_downloads::set(job) {
            Ok(()) => sync_auto(&ui),
            Err(e) => ui.report_error("Couldn't save the auto-download", &e),
        }
        if let Some(dialog) = dialog_ref.upgrade() {
            dialog.close();
        }
        ui.data_changed();
    });
    if let Some(stop) = stop {
        let (weak, dialog_ref) = (ui.downgrade(), dialog.downgrade());
        stop.connect_activated(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            match crate::auto_downloads::remove(&user_id, &series.id) {
                Ok(()) => ui.toast(&format!("Stopped auto-downloading {}", series.name)),
                Err(e) => ui.report_error("Couldn't stop auto-downloading", &e),
            }
            if let Some(dialog) = dialog_ref.upgrade() {
                dialog.close();
            }
            ui.data_changed();
        });
    }
    dialog.present(Some(parent));
}

/// Checks auto-downloads now, then every half hour while signed in.
pub fn start_auto(ui: &Ui) {
    sync_auto(ui);
    let weak = ui.downgrade();
    glib::timeout_add_seconds_local(30 * 60, move || match weak.upgrade() {
        Some(ui) => {
            sync_auto(&ui);
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

/// Checks auto-downloads shortly after playback stops, once Emby has the
/// new watch state.
pub fn sync_auto_soon(ui: &Ui) {
    let weak = ui.downgrade();
    let source = glib::timeout_add_seconds_local_once(15, move || {
        PENDING_SYNC.with(|p| p.take());
        if let Some(ui) = weak.upgrade() {
            sync_auto(&ui);
        }
    });
    if let Some(old) = PENDING_SYNC.with(|p| p.replace(Some(source))) {
        old.remove();
    }
}

/// Runs the auto-download jobs: queues the episodes each wants and deletes
/// watched downloads. A series the server can't list (offline, say) is
/// skipped quietly; the next check tries again.
pub fn sync_auto(ui: &Ui) {
    let jobs = crate::auto_downloads::for_user(&ui.user_id());
    if jobs.is_empty() || SYNCING.with(|s| s.replace(true)) {
        return;
    }
    let weak = ui.downgrade();
    let (client, user_id) = (ui.client(), ui.user_id());
    glib::spawn_future_local(async move {
        for job in jobs {
            let (client, user_id, series_id) =
                (client.clone(), user_id.clone(), job.series_id.clone());
            let episodes = crate::runtime::spawn_tokio(async move {
                client.series_episodes(&series_id, &user_id).await
            })
            .await;
            let Some(ui) = weak.upgrade() else {
                break;
            };
            let episodes = match episodes {
                Ok(episodes) => episodes,
                Err(e) => {
                    tracing::warn!("auto-download of {}: {e:#}", job.series_name);
                    continue;
                }
            };
            let downloaded: Vec<Download> = crate::downloads::list()
                .into_iter()
                .filter(|d| d.item.series_id.as_deref() == Some(job.series_id.as_str()))
                .collect();
            let playing = ui.playing_item_id();
            let plan =
                crate::auto_downloads::plan(&job, &episodes, &downloaded, playing.as_deref());
            let deleted = plan
                .delete
                .iter()
                .filter(|download| match crate::downloads::delete(download) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!("auto-download cleanup: {e:#}");
                        false
                    }
                })
                .count();
            let queued = plan.download.iter().filter(|e| enqueue(e)).count();
            let mut news = Vec::new();
            match queued {
                0 => {}
                1 => news.push("downloading 1 episode".to_string()),
                n => news.push(format!("downloading {n} episodes")),
            }
            match deleted {
                0 => {}
                1 => news.push("deleted 1 watched episode".to_string()),
                n => news.push(format!("deleted {n} watched episodes")),
            }
            if !news.is_empty() {
                ui.toast(&format!("{}: {}", job.series_name, news.join(", ")));
                pump(&ui);
                ui.data_changed();
            }
        }
        SYNCING.with(|s| s.set(false));
    });
}

/// The download button beside a movie's or episode's favourite star: plain
/// to download, a spinner while queued or downloading (opens Downloads),
/// green once downloaded (offers to delete it).
pub fn button(ui: &Ui, item: &BaseItem) -> Option<gtk::Button> {
    if !can_download(item) {
        return None;
    }
    let button = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .css_classes(["circular"])
        .build();
    let weak = ui.downgrade();
    let target = item.clone();
    if crate::downloads::is_downloaded(&item.id) {
        button.set_icon_name(crate::ui::icons::DOWNLOAD);
        button.add_css_class("downloaded");
        button.set_tooltip_text(Some("Downloaded; delete the download"));
        button.connect_clicked(move |button| {
            if let Some(ui) = weak.upgrade() {
                confirm_delete(&ui, &target, button);
            }
        });
    } else if is_running(&item.id) {
        button.set_child(Some(&adw::Spinner::new()));
        button.set_tooltip_text(Some("Downloading; show Downloads"));
        button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.open_downloads();
            }
        });
    } else {
        button.set_icon_name(crate::ui::icons::DOWNLOAD);
        button.set_tooltip_text(Some("Download"));
        button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                start(&ui, &target);
                ui.data_changed();
            }
        });
    }
    Some(button)
}

fn confirm_delete(ui: &Ui, item: &BaseItem, parent: &impl IsA<gtk::Widget>) {
    let message = adw::PreferencesGroup::builder()
        .title(glib::markup_escape_text(&title_of(item)))
        .description("Removes it from this device; it stays on the server.")
        .build();
    let delete = super::action_row("Delete Download", "destructive-action");
    let dialog = super::form_dialog(
        "Delete Download",
        &[&message, &super::actions_group(&[&delete])],
    );
    let (weak, target, dialog_ref) = (ui.downgrade(), item.clone(), dialog.downgrade());
    delete.connect_activated(move |_| {
        if let Some(dialog) = dialog_ref.upgrade() {
            dialog.close();
        }
        if let Some(ui) = weak.upgrade() {
            remove(&ui, &target);
        }
    });
    dialog.present(Some(parent));
}
