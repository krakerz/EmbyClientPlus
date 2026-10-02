//! An album: cover, artist, track list. Tracks play in album order.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::{Ui, clear, fixed_picture, format_runtime, format_timestamp, images, loading};
use super::{reload_on_change, scrolled_page};
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

const COVER: i32 = 240;

struct AlbumView {
    cover: gtk::Picture,
    title: gtk::Label,
    artist: gtk::Button,
    artist_handler: std::cell::RefCell<Option<glib::SignalHandlerId>>,
    meta: gtk::Label,
    buttons: gtk::Box,
    tracks: gtk::ListBox,
}

pub fn page(ui: &Ui, album: &BaseItem) -> adw::NavigationPage {
    let cover = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .build();
    let cover_frame = gtk::Overlay::builder()
        .child(&fixed_picture(&cover, COVER, COVER))
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Start)
        .css_classes(["card"])
        .build();
    let title = gtk::Label::builder()
        .label(&album.name)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-1"])
        .build();
    let artist = gtk::Button::builder()
        .halign(gtk::Align::Start)
        .css_classes(["flat", "heading"])
        .visible(false)
        .build();
    let meta = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let buttons = gtk::Box::builder().spacing(12).margin_top(6).build();
    let info = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .valign(gtk::Align::End)
        .hexpand(true)
        .build();
    info.append(&title);
    info.append(&artist);
    info.append(&meta);
    info.append(&buttons);
    let top = gtk::Box::builder().spacing(24).build();
    top.append(&cover_frame);
    top.append(&info);

    let tracks = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    tracks.append(&loading());
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .build();
    body.append(&top);
    body.append(&tracks);
    let content = adw::Clamp::builder()
        .maximum_size(960)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(18)
        .margin_end(18)
        .child(&body)
        .build();
    let page = scrolled_page(&album.name, None, &adw::HeaderBar::new(), &content);

    let view = Rc::new(AlbumView {
        cover,
        title,
        artist,
        artist_handler: std::cell::RefCell::new(None),
        meta,
        buttons,
        tracks,
    });
    let album_id = album.id.clone();
    reload_on_change(ui, &page, {
        let weak = ui.downgrade();
        let view = view.clone();
        let album_id = album_id.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &view, &album_id);
            }
        }
    });
    load(ui, &view, &album_id);
    page
}

fn load(ui: &Ui, view: &Rc<AlbumView>, album_id: &str) {
    let client = ui.client();
    let user_id = ui.user_id();
    let album_id = album_id.to_string();
    let ui = ui.clone();
    let view = view.clone();
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move {
            let (album, tracks) = tokio::join!(
                client.item(&user_id, &album_id),
                client.album_tracks(&user_id, &album_id),
            );
            Ok::<_, anyhow::Error>((album?, tracks?))
        })
        .await;
        match result {
            Ok((album, tracks)) => show(&ui, &view, &album, &tracks),
            Err(e) => {
                view.tracks.remove_all();
                ui.report_error("Could not load the album", &e);
            }
        }
    });
}

fn show(ui: &Ui, view: &AlbumView, album: &BaseItem, tracks: &[BaseItem]) {
    view.title.set_label(&album.name);
    images::load(ui, &view.cover, album.poster(), COVER as u32 * 2);

    let artist = album
        .album_artists
        .first()
        .or_else(|| album.artist_items.first());
    match artist {
        Some(artist) => {
            view.artist.set_label(&artist.name);
            view.artist.set_visible(true);
            let target = BaseItem {
                id: artist.id.clone(),
                name: artist.name.clone(),
                item_type: "MusicArtist".into(),
                ..Default::default()
            };
            if let Some(id) = view.artist_handler.take() {
                view.artist.disconnect(id);
            }
            let weak = ui.downgrade();
            let id = view.artist.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.open(&target);
                }
            });
            view.artist_handler.replace(Some(id));
        }
        None => view.artist.set_visible(false),
    }

    let runtime: i64 = tracks.iter().filter_map(|t| t.run_time_ticks).sum();
    let mut meta = Vec::new();
    if let Some(year) = album.production_year {
        meta.push(year.to_string());
    }
    meta.push(match tracks.len() {
        1 => "1 track".to_string(),
        n => format!("{n} tracks"),
    });
    if runtime > 0 {
        meta.push(format_runtime(runtime));
    }
    view.meta.set_label(&meta.join(" · "));

    clear(&view.buttons);
    if let Some(first) = tracks.first() {
        let play = gtk::Button::builder()
            .label("Play")
            .css_classes(["suggested-action", "pill"])
            .build();
        let weak = ui.downgrade();
        let first = first.clone();
        play.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.play(&first, 0);
            }
        });
        view.buttons.append(&play);
    }
    let [_, star] = super::hero::item_toggles(ui, album);
    view.buttons.append(&star);

    view.tracks.remove_all();
    let multi_disc = tracks
        .iter()
        .filter_map(|t| t.parent_index_number)
        .max()
        .is_some_and(|discs| discs > 1);
    for track in tracks {
        view.tracks.append(&track_row(ui, album, track, multi_disc));
    }
}

fn track_row(ui: &Ui, album: &BaseItem, track: &BaseItem, multi_disc: bool) -> adw::ActionRow {
    let number = match (multi_disc, track.parent_index_number, track.index_number) {
        (true, Some(disc), Some(n)) => format!("{disc}.{n}"),
        (_, _, Some(n)) => n.to_string(),
        _ => String::new(),
    };
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&track.name))
        .activatable(true)
        .build();
    // Only name the track's artist when it differs from the album's.
    if let Some(artist) = track.artist()
        && Some(artist) != album.artist()
    {
        row.set_subtitle(&glib::markup_escape_text(artist));
    }
    row.add_prefix(
        &gtk::Label::builder()
            .label(number)
            .width_chars(4)
            .xalign(1.0)
            .css_classes(["dim-label", "numeric"])
            .build(),
    );
    if let Some(ticks) = track.run_time_ticks {
        row.add_suffix(
            &gtk::Label::builder()
                .label(format_timestamp(ticks))
                .css_classes(["dim-label", "numeric"])
                .build(),
        );
    }
    let weak = ui.downgrade();
    let target = track.clone();
    row.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.play(&target, 0);
        }
    });
    row
}
