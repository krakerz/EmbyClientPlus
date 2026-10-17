//! Downloads: starting them (item menus), the Downloads page (running,
//! waiting and finished ones), and playing them from disk. One download
//! runs at a time; the rest wait in a queue (a whole season or series).

use std::cell::{Cell, RefCell};
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
    /// Stops it (a pause keeps the partial file to continue later)...
    cancelled: Arc<AtomicBool>,
    /// ...and a cancel deletes that file too.
    discard: Arc<AtomicBool>,
}

thread_local! {
    static RUNNING: RefCell<Vec<Running>> = const { RefCell::new(Vec::new()) };
    static QUEUE: RefCell<VecDeque<BaseItem>> = const { RefCell::new(VecDeque::new()) };
    /// Stopped downloads (paused, interrupted, or out of disk space),
    /// continued from where they stopped when resumed.
    static PAUSED: RefCell<Vec<BaseItem>> = const { RefCell::new(Vec::new()) };
    /// Bumped at sign-out: downloads started before it leave the lists alone.
    static EPOCH: Cell<u64> = const { Cell::new(0) };
    /// An auto-download check is under way.
    static SYNCING: Cell<bool> = const { Cell::new(false) };
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

fn is_paused(item_id: &str) -> bool {
    PAUSED.with(|p| p.borrow().iter().any(|i| i.id == item_id))
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

/// Queues `item` unless it's downloaded, downloading or queued already;
/// a paused one is resumed.
fn enqueue(item: &BaseItem) -> bool {
    // The cached id set: queueing a long series mustn't rescan the folder
    // for every episode.
    if is_running(&item.id) || crate::downloads::is_downloaded(&item.id) {
        return false;
    }
    let paused = PAUSED.with(|p| {
        let mut paused = p.borrow_mut();
        let index = paused.iter().position(|i| i.id == item.id)?;
        Some(paused.remove(index))
    });
    QUEUE.with(|q| {
        q.borrow_mut()
            .push_back(paused.unwrap_or_else(|| item.clone()))
    });
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
            client.downloadable_episodes(&series_id, &user_id).await
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

/// Starts the next queued download if none is running, and saves what's
/// left to do.
fn pump(ui: &Ui) {
    if RUNNING.with(|r| r.borrow().is_empty())
        && let Some(item) = QUEUE.with(|q| q.borrow_mut().pop_front())
    {
        run(ui, &item);
    }
    save_state(ui);
}

/// Saves the running, queued and paused downloads, so they survive the
/// app closing, even abruptly.
fn save_state(ui: &Ui) {
    save_state_for(&ui.user_id());
}

/// [`save_state`] for `user_id`, which needs no live [`Ui`].
fn save_state_for(user_id: &str) {
    let mut pending: Vec<crate::downloads::Pending> = Vec::new();
    RUNNING.with(|r| {
        pending.extend(
            r.borrow()
                .iter()
                .map(|d| crate::downloads::Pending::new(user_id, &d.item, false)),
        )
    });
    QUEUE.with(|q| {
        pending.extend(
            q.borrow()
                .iter()
                .map(|i| crate::downloads::Pending::new(user_id, i, false)),
        )
    });
    PAUSED.with(|p| {
        pending.extend(
            p.borrow()
                .iter()
                .map(|i| crate::downloads::Pending::new(user_id, i, true)),
        )
    });
    crate::downloads::save_pending(user_id, &pending);
}

/// At sign-out: stops the running download keeping its partial file (as
/// Pause does), saves everything under the signed-out user, and empties the
/// lists so the next user starts clean.
pub fn sign_out(ui: &Ui) {
    RUNNING.with(|r| {
        for d in r.borrow().iter() {
            d.cancelled.store(true, Ordering::Relaxed);
        }
    });
    save_state_for(&ui.user_id());
    EPOCH.with(|e| e.set(e.get() + 1));
    RUNNING.with(|r| r.borrow_mut().clear());
    QUEUE.with(|q| q.borrow_mut().clear());
    PAUSED.with(|p| p.borrow_mut().clear());
    if let Some(source) = PENDING_SYNC.with(|p| p.take()) {
        source.remove();
    }
}

/// At sign-in: the downloads left from last time. Queued ones carry on if
/// Preferences says so (else they wait, paused); paused ones stay paused.
pub fn restore(ui: &Ui) {
    let pending = crate::downloads::load_pending(&ui.user_id());
    if pending.is_empty() {
        return;
    }
    let resume = crate::config::Settings::load()
        .unwrap_or_default()
        .downloads
        .resume_at_start;
    let mut resumed = 0;
    for entry in pending {
        let item = entry.item();
        if crate::downloads::is_downloaded(&item.id) || is_running(&item.id) || is_paused(&item.id)
        {
            continue;
        }
        if resume && !entry.paused {
            QUEUE.with(|q| q.borrow_mut().push_back(item));
            resumed += 1;
        } else {
            PAUSED.with(|p| p.borrow_mut().push(item));
        }
    }
    match resumed {
        0 => {}
        1 => ui.toast("Resuming 1 download"),
        n => ui.toast(&format!("Resuming {n} downloads")),
    }
    pump(ui);
}

fn run(ui: &Ui, item: &BaseItem) {
    let running = Running {
        item: item.clone(),
        progress: Arc::new(Progress::default()),
        cancelled: Arc::new(AtomicBool::new(false)),
        discard: Arc::new(AtomicBool::new(false)),
    };
    RUNNING.with(|r| r.borrow_mut().push(running.clone()));
    let (client, user_id) = (ui.client(), ui.user_id());
    let weak = ui.downgrade();
    let epoch = EPOCH.with(Cell::get);
    glib::spawn_future_local(async move {
        let (progress, cancelled, discard, id) = (
            running.progress.clone(),
            running.cancelled.clone(),
            running.discard.clone(),
            running.item.id.clone(),
        );
        let result = crate::runtime::spawn_tokio(crate::downloads::fetch(
            client,
            user_id.clone(),
            id,
            progress,
            cancelled,
            discard,
        ))
        .await;
        // Signed out meanwhile: `sign_out` saved it, and the lists are the
        // next user's now.
        if EPOCH.with(Cell::get) != epoch {
            return;
        }
        RUNNING.with(|r| r.borrow_mut().retain(|d| d.item.id != running.item.id));
        let name = running.item.episode_label();
        let pause = |item: BaseItem| PAUSED.with(|p| p.borrow_mut().push(item));
        // The bookkeeping doesn't need the Ui (it may be gone); only the
        // messages do.
        let mut toast = None;
        let mut failure = None;
        match &result {
            Ok(_) => toast = Some(format!("Downloaded {name}")),
            Err(_) if running.discard.load(Ordering::Relaxed) => {}
            Err(_) if running.cancelled.load(Ordering::Relaxed) => pause(running.item.clone()),
            Err(e) => {
                // Interrupted (connection lost, server gone) or out of
                // space: resumable.
                pause(running.item.clone());
                // Out of space: everything waiting pauses too, rather than
                // failing one by one.
                if let Some(space) = e.downcast_ref::<crate::downloads::NotEnoughSpace>() {
                    let waiting: Vec<BaseItem> = QUEUE.with(|q| q.borrow_mut().drain(..).collect());
                    PAUSED.with(|p| p.borrow_mut().extend(waiting));
                    toast = Some(format!(
                        "Downloads paused: not enough disk space ({:.1} GB needed, {:.1} GB free)",
                        space.needed as f64 / 1e9,
                        space.free as f64 / 1e9
                    ));
                } else {
                    failure = Some(e);
                }
            }
        }
        save_state_for(&user_id);
        let Some(ui) = weak.upgrade() else { return };
        if let Some(toast) = toast {
            ui.toast(&toast);
        }
        if let Some(e) = failure {
            ui.report_error(&format!("Couldn't finish downloading {name}"), e);
        }
        pump(&ui);
        ui.data_changed();
    });
}

/// Pauses the running download `item_id`, keeping what's downloaded.
fn pause(item_id: &str) {
    RUNNING.with(|r| {
        if let Some(d) = r.borrow().iter().find(|d| d.item.id == item_id) {
            d.cancelled.store(true, Ordering::Relaxed);
        }
    });
}

/// Cancels the running download `item_id` and deletes its partial file.
fn cancel_running(item_id: &str) {
    RUNNING.with(|r| {
        if let Some(d) = r.borrow().iter().find(|d| d.item.id == item_id) {
            d.discard.store(true, Ordering::Relaxed);
            d.cancelled.store(true, Ordering::Relaxed);
        }
    });
}

/// Resumes paused downloads (all of them, or just `item_id`).
fn resume(ui: &Ui, item_id: Option<&str>) {
    let items: Vec<BaseItem> = PAUSED.with(|p| {
        let mut paused = p.borrow_mut();
        let (chosen, kept) = paused
            .drain(..)
            .partition(|i| item_id.is_none_or(|id| i.id == id));
        *paused = kept;
        chosen
    });
    QUEUE.with(|q| q.borrow_mut().extend(items));
    pump(ui);
    ui.data_changed();
}

/// Drops a paused download and its partial file.
fn cancel_paused(ui: &Ui, item_id: &str) {
    PAUSED.with(|p| p.borrow_mut().retain(|i| i.id != item_id));
    crate::downloads::discard_partial(item_id);
    save_state(ui);
    ui.data_changed();
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
    // Running downloads' progress, while the page is showing (not hidden,
    // as when the player is pushed over it).
    let ticker: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let start = Rc::new({
        let (ticker, content, fill) = (ticker.clone(), content.downgrade(), fill.clone());
        move || {
            if ticker.borrow().is_some() {
                return;
            }
            let (content, fill) = (content.clone(), fill.clone());
            let shown = RefCell::new(running_ids());
            let source =
                glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
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
                });
            ticker.replace(Some(source));
        }
    });
    start();
    page.connect_showing({
        let (ticker, fill) = (ticker.clone(), fill.clone());
        move |_| {
            // Back from the player: what happened meanwhile shows now.
            if ticker.borrow().is_none() {
                fill();
                start();
            }
        }
    });
    page.connect_hidden(move |_| {
        if let Some(source) = ticker.take() {
            source.remove();
        }
    });
    page
}

/// What's downloading, waiting and paused: the page rebuilds when this
/// changes.
fn running_ids() -> Vec<String> {
    let mut ids: Vec<String> =
        RUNNING.with(|r| r.borrow().iter().map(|d| d.item.id.clone()).collect());
    QUEUE.with(|q| ids.extend(q.borrow().iter().map(|i| i.id.clone())));
    ids.push(String::new());
    PAUSED.with(|p| ids.extend(p.borrow().iter().map(|i| i.id.clone())));
    ids
}

const PROGRESS_KEY: &str = "embyclientplus-download-progress";

fn fill(ui: &Ui, content: &gtk::Box) {
    let mark = super::gamepad::mark_cursor(content);
    clear(content);
    let running: Vec<Running> = RUNNING.with(|r| r.borrow().clone());
    let waiting: Vec<BaseItem> = QUEUE.with(|q| q.borrow().iter().cloned().collect());
    let paused: Vec<BaseItem> = PAUSED.with(|p| p.borrow().clone());
    let finished = crate::downloads::list();
    let folder = crate::downloads::folder();
    let used: u64 = finished.iter().map(Download::size).sum();
    let mut storage = format!(
        "Saved in {} · {} used",
        folder.display(),
        super::format_size(used)
    );
    if let Some(free) = crate::downloads::free_space(&folder) {
        storage.push_str(&format!(" · {} free", super::format_size(free)));
    }
    content.append(
        &gtk::Label::builder()
            .label(storage)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    let jobs = crate::auto_downloads::for_user(&ui.user_id());
    if running.is_empty()
        && waiting.is_empty()
        && paused.is_empty()
        && finished.is_empty()
        && jobs.is_empty()
    {
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
        let list = section(content, "Auto-download", None);
        for job in &jobs {
            list.append(&job_row(ui, job));
        }
    }
    if !running.is_empty() {
        let list = section(content, "Downloading", None);
        for download in running {
            list.append(&running_row(&download));
        }
    }
    if !waiting.is_empty() {
        let list = section(content, "Waiting", None);
        for item in waiting {
            list.append(&waiting_row(ui, &item));
        }
    }
    if !paused.is_empty() {
        let resume_all = gtk::Button::builder()
            .label("Resume All")
            .css_classes(["flat"])
            .build();
        let weak = ui.downgrade();
        resume_all.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                resume(&ui, None);
            }
        });
        let list = section(content, "Paused", Some(resume_all.upcast_ref()));
        for item in paused {
            list.append(&paused_row(ui, &item));
        }
    }
    // Downloaded, a section per series (with its size), movies last.
    let mut groups: Vec<(String, Vec<Download>)> = Vec::new();
    for download in finished {
        let name = match (&download.item.series_name, download.item.item_type.as_str()) {
            (Some(series), "Episode") => series.clone(),
            _ => "Movies and Videos".to_string(),
        };
        match groups.iter_mut().find(|(n, _)| *n == name) {
            Some((_, list)) => list.push(download),
            None => groups.push((name, vec![download])),
        }
    }
    groups.sort_by_key(|(name, _)| name == "Movies and Videos");
    for (name, downloads) in groups {
        let size: u64 = downloads.iter().map(Download::size).sum();
        let title = format!("{name} · {}", super::format_size(size));
        let list = section(content, &title, None);
        for download in downloads {
            list.append(&finished_row(ui, &download));
        }
    }
    if let Some(mark) = mark {
        mark.restore();
    }
}

/// A heading (with an optional button at its end) over a list.
fn section(content: &gtk::Box, title: &str, action: Option<&gtk::Widget>) -> gtk::ListBox {
    let heading = gtk::Box::builder().spacing(12).build();
    heading.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["heading"])
            .build(),
    );
    if let Some(action) = action {
        heading.append(action);
    }
    content.append(&heading);
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    content.append(&list);
    list
}

/// A flat icon button for a row's end.
fn row_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
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
    let pause_button = row_button(crate::ui::icons::PAUSE, "Pause (keeps what's downloaded)");
    let id = download.item.id.clone();
    pause_button.connect_clicked(move |_| pause(&id));
    row.add_suffix(&pause_button);
    // A on the row (controller) pauses, like its button.
    row.set_activatable(true);
    let id = download.item.id.clone();
    row.connect_activated(move |_| pause(&id));
    let cancel = row_button(
        crate::ui::icons::REMOVE,
        "Cancel and delete the partial download",
    );
    let id = download.item.id.clone();
    cancel.connect_clicked(move |_| cancel_running(&id));
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
    let cancel = row_button(crate::ui::icons::REMOVE, "Don't download");
    let (weak, id) = (ui.downgrade(), item.id.clone());
    cancel.connect_clicked(move |_| {
        QUEUE.with(|q| q.borrow_mut().retain(|i| i.id != id));
        if let Some(ui) = weak.upgrade() {
            save_state(&ui);
            ui.data_changed();
        }
    });
    row.add_suffix(&cancel);
    row
}

fn paused_row(ui: &Ui, item: &BaseItem) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&title_of(item)))
        .subtitle("Continues where it stopped")
        .activatable(true)
        .build();
    // A on the row (controller) resumes, like its button.
    let (weak, id) = (ui.downgrade(), item.id.clone());
    row.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            resume(&ui, Some(&id));
        }
    });
    let play = row_button(crate::ui::icons::PLAY, "Resume");
    let (weak, id) = (ui.downgrade(), item.id.clone());
    play.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            resume(&ui, Some(&id));
        }
    });
    row.add_suffix(&play);
    let cancel = row_button(
        crate::ui::icons::REMOVE,
        "Cancel and delete the partial download",
    );
    let (weak, id) = (ui.downgrade(), item.id.clone());
    cancel.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            cancel_paused(&ui, &id);
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

/// At sign-in: carries on with last time's downloads, then checks
/// auto-downloads now and every half hour while signed in.
pub fn start_auto(ui: &Ui) {
    restore(ui);
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
                client.downloadable_episodes(&series_id, &user_id).await
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
            // A paused episode stays paused (by you, or for disk space).
            let queued = plan
                .download
                .iter()
                .filter(|e| !is_paused(&e.id) && enqueue(e))
                .count();
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
