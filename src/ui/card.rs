//! The poster/thumbnail card used by home rows and grids.

use gtk::glib;
use gtk::prelude::*;

use super::{Ui, images};
use crate::emby::browse::{ImageRef, ItemQuery};
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 2:3 poster (movies, series).
    Poster,
    /// 16:9 still (episodes, libraries, continue watching).
    Landscape,
    /// Round headshot (cast).
    Person,
    /// Square cover (music).
    Square,
    /// Like `Landscape`, but episodes show series art instead of their
    /// still (spoiler-free Continue Watching / Next Up).
    SeriesLandscape,
}

impl Shape {
    /// The shape for Continue Watching / Next Up, per Preferences.
    pub fn for_episodes() -> Shape {
        match crate::config::Settings::load()
            .unwrap_or_default()
            .home
            .episode_art
        {
            crate::config::EpisodeArt::Episode => Shape::Landscape,
            crate::config::EpisodeArt::Series => Shape::SeriesLandscape,
        }
    }
}

impl Shape {
    pub fn size(self) -> (i32, i32) {
        match self {
            Shape::Poster => (150, 225),
            Shape::Landscape | Shape::SeriesLandscape => (260, 146),
            Shape::Person => (120, 120),
            Shape::Square => (180, 180),
        }
    }
}

/// A card's widgets. `root` is what gets placed in a container; `from_root`
/// recovers the parts from it, for grid cells that only hand back widgets.
pub struct Card {
    pub root: gtk::Box,
    picture: gtk::Picture,
    progress: gtk::ProgressBar,
    placeholder: gtk::Image,
    title: gtk::Label,
    /// The line under the title: subtitle (year, episode), then the status
    /// at its end.
    details: gtk::Box,
    subtitle: gtk::Label,
    /// Unwatched episode count (series, seasons).
    unplayed: gtk::Label,
    /// Watched: a check in a circle, in place of the count.
    watched: gtk::Image,
}

impl Card {
    pub fn new(shape: Shape) -> Self {
        let (width, height) = shape.size();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .build();
        let progress = gtk::ProgressBar::builder()
            .valign(gtk::Align::End)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .visible(false)
            .build();
        let frame = gtk::Overlay::builder()
            .child(&super::fixed_picture(&picture, width, height))
            .overflow(gtk::Overflow::Hidden)
            .css_classes(if shape == Shape::Person {
                vec!["card", "person-card"]
            } else {
                vec!["card"]
            })
            .build();
        frame.add_overlay(&progress);
        let placeholder = super::placeholder_for(&picture, crate::ui::icons::IMAGE);
        frame.add_overlay(&placeholder);

        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .margin_top(6)
            .css_classes(["card-title"])
            .build();
        let subtitle = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .css_classes(["dim-label", "caption"])
            .build();
        let unplayed = gtk::Label::builder()
            .css_classes(["unplayed-count", "caption", "numeric"])
            .visible(false)
            .build();
        let watched = gtk::Image::builder()
            .icon_name(crate::ui::icons::WATCHED)
            .valign(gtk::Align::Center)
            .css_classes(["watched-mark"])
            .visible(false)
            .build();
        let details = gtk::Box::builder().spacing(6).build();
        details.append(&subtitle);
        details.append(&unplayed);
        details.append(&watched);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(super::scaled(width))
            .halign(gtk::Align::Center)
            .build();
        root.append(&frame);
        root.append(&title);
        root.append(&details);
        Card {
            root,
            picture,
            progress,
            placeholder,
            title,
            details,
            subtitle,
            unplayed,
            watched,
        }
    }

    pub fn from_root(root: &gtk::Box) -> Self {
        let frame: gtk::Overlay = child(root.first_child());
        let sized = frame.child().expect("card frame has a picture");
        let picture = super::fixed_picture_child(&sized).expect("card picture is pinned");
        let progress: gtk::ProgressBar = child(sized.next_sibling());
        let placeholder: gtk::Image = child(progress.next_sibling());
        let title: gtk::Label = child(frame.next_sibling());
        let details: gtk::Box = child(title.next_sibling());
        let subtitle: gtk::Label = child(details.first_child());
        let unplayed: gtk::Label = child(subtitle.next_sibling());
        let watched: gtk::Image = child(unplayed.next_sibling());
        Card {
            root: root.clone(),
            picture,
            progress,
            placeholder,
            title,
            details,
            subtitle,
            unplayed,
            watched,
        }
    }

    pub fn bind(&self, ui: &Ui, item: &BaseItem, shape: Shape) {
        let (title, subtitle) = labels(item, shape);
        self.title.set_label(&title);
        self.subtitle.set_label(&subtitle);
        match item.progress() {
            Some(fraction) => {
                self.progress.set_fraction(fraction);
                self.progress.set_visible(true);
            }
            None => self.progress.set_visible(false),
        }
        let played = item.played();
        let unplayed = item.unplayed_count();
        self.watched.set_visible(played);
        self.unplayed.set_label(&if unplayed >= 1000 {
            "1k+".to_string()
        } else {
            unplayed.to_string()
        });
        self.unplayed.set_visible(unplayed > 0 && !played);
        self.details
            .set_visible(!subtitle.is_empty() || self.unplayed.is_visible() || played);
        self.placeholder
            .set_icon_name(Some(crate::ui::icons::placeholder(&item.item_type)));
        let (image, width) = match shape {
            Shape::Poster | Shape::Person | Shape::Square => (item.poster(), 300),
            Shape::Landscape => (item.landscape().or_else(|| item.poster()), 520),
            Shape::SeriesLandscape => (item.series_landscape().or_else(|| item.poster()), 520),
        };
        if image.is_none() && item.is_folder() {
            folder_cover(ui, &self.picture, &item.id, width);
        } else {
            // A recycled cell must not take a cover still loading for the
            // folder it showed before.
            // SAFETY: `FOLDER_KEY` is only ever stored and read as `String`.
            unsafe { self.picture.set_data(FOLDER_KEY, String::new()) };
            images::load(ui, &self.picture, image, width);
        }
    }
}

/// Right-click / long-press menu for a card: play, watched, favourite,
/// series. `item` is read at open time, since grid cells get recycled.
pub fn attach_menu(
    ui: &Ui,
    widget: &impl IsA<gtk::Widget>,
    item: impl Fn() -> Option<BaseItem> + 'static,
) {
    let item = std::rc::Rc::new(item);
    let open = {
        let ui = ui.downgrade();
        let widget = widget.as_ref().downgrade();
        move || {
            if let (Some(ui), Some(widget), Some(item)) = (ui.upgrade(), widget.upgrade(), item()) {
                show_menu(&ui, &widget, &item);
            }
        }
    };
    let open = std::rc::Rc::new(open);
    super::set_menu_opener(widget.as_ref(), {
        let open = open.clone();
        move |()| open()
    });
    let right_click = gtk::GestureClick::builder()
        .button(gtk::gdk::BUTTON_SECONDARY)
        .build();
    right_click.connect_pressed({
        let open = open.clone();
        move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            open();
        }
    });
    widget.add_controller(right_click);
    let long_press = gtk::GestureLongPress::new();
    long_press.connect_pressed(move |gesture, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        open();
    });
    widget.add_controller(long_press);
}

/// Opens the card menu for `item` anchored on `widget` (also used by the
/// controller's context-menu button).
pub fn show_menu(ui: &Ui, widget: &gtk::Widget, item: &BaseItem) {
    let entries = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    let popover = gtk::Popover::builder()
        .child(&entries)
        .has_arrow(true)
        .build();
    popover.set_parent(widget);
    popover.connect_closed(|popover| {
        // Parented per opening; let it go once closed.
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    let add = |label: &str, action: Box<dyn Fn(&Ui)>| {
        let button = gtk::Button::builder()
            .label(label)
            .css_classes(["flat"])
            .build();
        if let Some(child) = button.child().and_downcast::<gtk::Label>() {
            child.set_xalign(0.0);
        }
        let ui = ui.downgrade();
        let popover = popover.downgrade();
        button.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            if let Some(ui) = ui.upgrade() {
                action(&ui);
            }
        });
        entries.append(&button);
    };
    if super::playlists::is_music(item) {
        if !item.is_audio() {
            let target = item.clone();
            add(
                "Play",
                Box::new(move |ui| ui.play_collection(&target, false)),
            );
            let target = item.clone();
            add(
                "Shuffle",
                Box::new(move |ui| ui.play_collection(&target, true)),
            );
        }
        if item.item_type != "Playlist" {
            let target = item.clone();
            add(
                "Instant Mix",
                Box::new(move |ui| ui.play_instant_mix(&target)),
            );
        }
        let target = item.clone();
        add(
            "Play Next",
            Box::new(move |ui| ui.queue_music(&target, true)),
        );
        let target = item.clone();
        add(
            "Add to Queue",
            Box::new(move |ui| ui.queue_music(&target, false)),
        );
        if item.item_type != "Playlist" {
            let target = item.clone();
            add(
                "Add to Playlist…",
                Box::new(move |ui| ui.add_to_playlist(&target)),
            );
        }
    }
    if item.is_playable() {
        let resume = item.resume_ticks();
        let target = item.clone();
        let label = if resume > 0 { "Resume" } else { "Play" };
        add(label, Box::new(move |ui| ui.play(&target, resume)));
    }
    let target = item.clone();
    add("Open", Box::new(move |ui| ui.open(&target)));
    if item.item_type != "Person" && !super::playlists::is_music(item) {
        let played = item.played();
        let target = item.clone();
        add(
            if played {
                "Mark as Unwatched"
            } else {
                "Mark as Watched"
            },
            Box::new(move |ui| ui.set_played(&target, !played)),
        );
    }
    let favorite = item.is_favorite();
    let target = item.clone();
    add(
        if favorite {
            "Remove from Favourites"
        } else {
            "Add to Favourites"
        },
        Box::new(move |ui| ui.set_favorite(&target, !favorite)),
    );
    if let (Some(series_id), Some(series_name)) = (&item.series_id, &item.series_name) {
        let series = BaseItem {
            id: series_id.clone(),
            name: series_name.clone(),
            item_type: "Series".into(),
            ..Default::default()
        };
        add("Go to Series", Box::new(move |ui| ui.open(&series)));
    }
    popover.popup();
}

thread_local! {
    /// Folder id → the cover borrowed from its first title (`None`: none).
    static FOLDER_COVERS: std::cell::RefCell<std::collections::HashMap<String, Option<ImageRef>>> =
        Default::default();
}

const FOLDER_KEY: &str = "embyclientplus-folder-cover";

/// A folder without its own image shows its first title's cover, the way
/// Emby's web client does.
fn folder_cover(ui: &Ui, picture: &gtk::Picture, folder_id: &str, width: u32) {
    if let Some(cached) = FOLDER_COVERS.with(|c| c.borrow().get(folder_id).cloned()) {
        images::load(ui, picture, cached, width);
        return;
    }
    images::load(ui, picture, None, width);
    // SAFETY: this key is only ever stored and read as `String`.
    unsafe { picture.set_data(FOLDER_KEY, folder_id.to_string()) };
    let client = ui.client();
    let user_id = ui.user_id();
    let query = ItemQuery {
        parent_id: Some(folder_id.to_string()),
        include_types: Some("Series,Movie"),
        recursive: true,
        image_types: Some("Primary"),
        limit: 1,
        ..Default::default()
    };
    let folder_id = folder_id.to_string();
    let weak_ui = ui.downgrade();
    let picture = picture.downgrade();
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move { client.items(&user_id, &query).await }).await;
        let cover = match result {
            Ok(page) => page.items.first().and_then(BaseItem::poster),
            Err(e) => {
                tracing::debug!("folder cover for {folder_id} failed: {e:#}");
                return;
            }
        };
        FOLDER_COVERS.with(|c| c.borrow_mut().insert(folder_id.clone(), cover.clone()));
        let (Some(ui), Some(picture)) = (weak_ui.upgrade(), picture.upgrade()) else {
            return;
        };
        // SAFETY: see above; the cell may have been recycled meanwhile.
        let still_wanted = unsafe {
            picture
                .data::<String>(FOLDER_KEY)
                .is_some_and(|id| *id.as_ref() == folder_id)
        };
        if still_wanted {
            images::load(&ui, &picture, cover, width);
        }
    });
}

/// A card wrapped in a flat button, for home rows.
pub fn button(
    ui: &Ui,
    item: &BaseItem,
    shape: Shape,
    on_click: impl Fn() + 'static,
) -> gtk::Button {
    let card = Card::new(shape);
    card.bind(ui, item, shape);
    let button = gtk::Button::builder()
        .child(&card.root)
        .css_classes(["flat", "card-button"])
        .build();
    button.connect_clicked(move |_| on_click());
    let target = item.clone();
    attach_menu(ui, &button, move || Some(target.clone()));
    button
}

/// Title and secondary line for a card: episodes show their series name
/// over "S1:E4 · Name"; everything else shows its name over the year.
fn labels(item: &BaseItem, shape: Shape) -> (String, String) {
    if item.item_type == "Person" {
        return (item.name.clone(), item.role.clone().unwrap_or_default());
    }
    if matches!(item.item_type.as_str(), "MusicAlbum" | "Audio") {
        return (
            item.name.clone(),
            item.artist().unwrap_or_default().to_string(),
        );
    }
    match (item.item_type.as_str(), &item.series_name) {
        ("Episode", Some(series)) if matches!(shape, Shape::Landscape | Shape::SeriesLandscape) => {
            (series.clone(), item.episode_label())
        }
        _ => (
            item.name.clone(),
            item.production_year
                .map(|year| year.to_string())
                .unwrap_or_default(),
        ),
    }
}

fn child<T: IsA<gtk::Widget>>(widget: Option<gtk::Widget>) -> T {
    widget
        .and_then(|widget| widget.downcast().ok())
        .expect("card widget tree has an unexpected shape")
}
