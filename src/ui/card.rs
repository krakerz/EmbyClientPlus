//! The poster/thumbnail card used by home rows and grids.

use gtk::prelude::*;

use super::{Ui, images};
use crate::emby::models::BaseItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 2:3 poster (movies, series).
    Poster,
    /// 16:9 still (episodes, libraries, continue watching).
    Landscape,
}

impl Shape {
    fn size(self) -> (i32, i32) {
        match self {
            Shape::Poster => (150, 225),
            Shape::Landscape => (260, 146),
        }
    }
}

/// A card's widgets. `root` is what gets placed in a container; `from_root`
/// recovers the parts from it, for grid cells that only hand back widgets.
pub struct Card {
    pub root: gtk::Box,
    picture: gtk::Picture,
    progress: gtk::ProgressBar,
    watched: gtk::Image,
    title: gtk::Label,
    subtitle: gtk::Label,
}

impl Card {
    pub fn new(shape: Shape) -> Self {
        let (width, height) = shape.size();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .width_request(width)
            .height_request(height)
            .build();
        let progress = gtk::ProgressBar::builder()
            .valign(gtk::Align::End)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .visible(false)
            .build();
        let watched = gtk::Image::builder()
            .icon_name("object-select-symbolic")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(6)
            .margin_end(6)
            .css_classes(["watched-badge"])
            .visible(false)
            .build();
        let frame = gtk::Overlay::builder()
            .child(&picture)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["card"])
            .build();
        frame.add_overlay(&progress);
        frame.add_overlay(&watched);

        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .margin_top(6)
            .build();
        let subtitle = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .css_classes(["dim-label", "caption"])
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(width)
            .halign(gtk::Align::Center)
            .build();
        root.append(&frame);
        root.append(&title);
        root.append(&subtitle);
        Card {
            root,
            picture,
            progress,
            watched,
            title,
            subtitle,
        }
    }

    pub fn from_root(root: &gtk::Box) -> Self {
        let frame: gtk::Overlay = child(root.first_child());
        let picture: gtk::Picture = child(frame.child());
        let progress: gtk::ProgressBar = child(picture.next_sibling());
        let watched: gtk::Image = child(progress.next_sibling());
        let title: gtk::Label = child(frame.next_sibling());
        let subtitle: gtk::Label = child(title.next_sibling());
        Card {
            root: root.clone(),
            picture,
            progress,
            watched,
            title,
            subtitle,
        }
    }

    pub fn bind(&self, ui: &Ui, item: &BaseItem, shape: Shape) {
        let (title, subtitle) = labels(item, shape);
        self.title.set_label(&title);
        self.subtitle.set_label(&subtitle);
        self.subtitle.set_visible(!subtitle.is_empty());
        match item.progress() {
            Some(fraction) => {
                self.progress.set_fraction(fraction);
                self.progress.set_visible(true);
            }
            None => self.progress.set_visible(false),
        }
        self.watched.set_visible(item.played());
        let (image, width) = match shape {
            Shape::Poster => (item.poster(), 300),
            Shape::Landscape => (item.landscape().or_else(|| item.poster()), 520),
        };
        images::load(ui, &self.picture, image, width);
    }
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
    button
}

/// Title and secondary line for a card: episodes show their series name
/// over "S1:E4 · Name"; everything else shows its name over the year.
fn labels(item: &BaseItem, shape: Shape) -> (String, String) {
    match (item.item_type.as_str(), &item.series_name) {
        ("Episode", Some(series)) if shape == Shape::Landscape => {
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
