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
    if running.is_empty() && waiting.is_empty() && finished.is_empty() {
        content.append(
            &adw::StatusPage::builder()
                .icon_name(crate::ui::icons::DOWNLOAD)
                .title("No downloads")
                .description(
                    "Choose Download in a movie's, episode's, season's or series' menu to watch \
                     it offline",
                )
                .vexpand(true)
                .build(),
        );
        return;
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
