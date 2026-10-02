//! Music playlists: the Playlists tab, a playlist's page, and the
//! "Add to playlist" dialog. Also how a song/album/artist/playlist turns
//! into a list of tracks for the music queue.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::card::Shape;
use super::{Ui, clear, fixed_picture, format_runtime, format_timestamp, icons, images, loading};
use super::{library, reload_on_change, scrolled_page};
use crate::emby::EmbyClient;
use crate::emby::browse::ItemQuery;
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

const COVER: i32 = 240;
/// Upper bound on tracks queued from an artist or folder at once.
const MAX_TRACKS: usize = 500;

/// The tracks `item` stands for, in play order.
pub async fn tracks_of(
    client: &EmbyClient,
    user_id: &str,
    item: &BaseItem,
) -> anyhow::Result<Vec<BaseItem>> {
    match item.item_type.as_str() {
        "Audio" => Ok(vec![item.clone()]),
        "MusicAlbum" => client.album_tracks(user_id, &item.id).await,
        "Playlist" => client.playlist_items(user_id, &item.id).await,
        kind => {
            let query = ItemQuery {
                artist_id: (kind == "MusicArtist").then(|| item.id.clone()),
                parent_id: (kind != "MusicArtist").then(|| item.id.clone()),
                include_types: Some("Audio"),
                recursive: true,
                sort_by: Some("Album,ParentIndexNumber,IndexNumber,SortName"),
                limit: MAX_TRACKS,
                ..Default::default()
            };
            Ok(client.items(user_id, &query).await?.items)
        }
    }
}

/// Whether `item` holds music the queue can take.
pub fn is_music(item: &BaseItem) -> bool {
    matches!(
        item.item_type.as_str(),
        "Audio" | "MusicAlbum" | "MusicArtist" | "Playlist"
    )
}

/// The Playlists tab: every music playlist of the user.
pub fn grid(ui: &Ui) -> gtk::Widget {
    let (grid, store) = library::grid(ui, Shape::Square);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .vexpand(true)
        .build();
    let (client, user_id, weak) = (ui.client(), ui.user_id(), ui.downgrade());
    glib::spawn_future_local(async move {
        match spawn_tokio(async move { client.playlists(&user_id).await }).await {
            Ok(playlists) if playlists.is_empty() => {
                if let Some(ui) = weak.upgrade() {
                    ui.toast("No playlists yet: add songs with “Add to Playlist…”");
                }
            }
            Ok(playlists) => {
                let objects: Vec<_> = playlists
                    .into_iter()
                    .map(glib::BoxedAnyObject::new)
                    .collect();
                store.extend_from_slice(&objects);
            }
            Err(e) => {
                if let Some(ui) = weak.upgrade() {
                    ui.report_error("Could not load playlists", &e);
                }
            }
        }
    });
    scrolled.upcast()
}

struct PlaylistView {
    cover: gtk::Picture,
    meta: gtk::Label,
    buttons: gtk::Box,
    tracks: gtk::ListBox,
}

pub fn page(ui: &Ui, playlist: &BaseItem) -> adw::NavigationPage {
    let cover = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .build();
    let cover_frame = gtk::Overlay::builder()
        .child(&fixed_picture(&cover, COVER, COVER))
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Start)
        .css_classes(["card"])
        .build();
    cover_frame.add_overlay(&super::placeholder_for(&cover, icons::PLAYLIST));
    let title = gtk::Label::builder()
        .label(&playlist.name)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-1"])
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
    let page = scrolled_page(&playlist.name, None, &adw::HeaderBar::new(), &content);
    images::load(ui, &cover, playlist.poster(), COVER as u32 * 2);

    let view = Rc::new(PlaylistView {
        cover,
        meta,
        buttons,
        tracks,
    });
    let playlist = playlist.clone();
    reload_on_change(ui, &page, {
        let (weak, view, playlist) = (ui.downgrade(), view.clone(), playlist.clone());
        move || {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &view, &playlist);
            }
        }
    });
    load(ui, &view, &playlist);
    page
}

fn load(ui: &Ui, view: &Rc<PlaylistView>, playlist: &BaseItem) {
    let (client, user_id, id) = (ui.client(), ui.user_id(), playlist.id.clone());
    let (ui, view, playlist) = (ui.clone(), view.clone(), playlist.clone());
    glib::spawn_future_local(async move {
        match spawn_tokio(async move { client.playlist_items(&user_id, &id).await }).await {
            Ok(tracks) => show(&ui, &view, &playlist, &tracks),
            Err(e) => {
                view.tracks.remove_all();
                ui.report_error("Could not load the playlist", &e);
            }
        }
    });
}

fn show(ui: &Ui, view: &Rc<PlaylistView>, playlist: &BaseItem, tracks: &[BaseItem]) {
    if playlist.poster().is_none()
        && let Some(first) = tracks.first()
    {
        images::load(ui, &view.cover, first.poster(), COVER as u32 * 2);
    }
    let runtime: i64 = tracks.iter().filter_map(|t| t.run_time_ticks).sum();
    let mut meta = vec![match tracks.len() {
        1 => "1 track".to_string(),
        n => format!("{n} tracks"),
    }];
    if runtime > 0 {
        meta.push(format_runtime(runtime));
    }
    view.meta.set_label(&meta.join(" · "));

    clear(&view.buttons);
    if !tracks.is_empty() {
        for (label, shuffle) in [("Play", false), ("Shuffle", true)] {
            let button = gtk::Button::builder()
                .label(label)
                .css_classes(if shuffle {
                    vec!["pill"]
                } else {
                    vec!["suggested-action", "pill"]
                })
                .build();
            let (weak, tracks) = (ui.downgrade(), tracks.to_vec());
            button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.play_music(tracks.clone(), 0, shuffle);
                }
            });
            view.buttons.append(&button);
        }
    }

    view.tracks.remove_all();
    let tracks: Rc<[BaseItem]> = tracks.into();
    for index in 0..tracks.len() {
        view.tracks
            .append(&track_row(ui, view, playlist, &tracks, index));
    }
}

fn track_row(
    ui: &Ui,
    view: &Rc<PlaylistView>,
    playlist: &BaseItem,
    tracks: &Rc<[BaseItem]>,
    index: usize,
) -> adw::ActionRow {
    let track = &tracks[index];
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&track.name))
        .subtitle(glib::markup_escape_text(
            &[track.artist(), track.album.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
        ))
        .activatable(true)
        .build();
    if let Some(ticks) = track.run_time_ticks {
        row.add_suffix(
            &gtk::Label::builder()
                .label(format_timestamp(ticks))
                .css_classes(["dim-label", "numeric"])
                .build(),
        );
    }
    let edit = |icon: &str, tooltip: &str, sensitive: bool, change: Edit| {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .valign(gtk::Align::Center)
            .sensitive(sensitive && track.playlist_item_id.is_some())
            .css_classes(["flat", "circular"])
            .build();
        let (weak, view, playlist) = (ui.downgrade(), view.clone(), playlist.clone());
        let entry = track.playlist_item_id.clone().unwrap_or_default();
        button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                edit_playlist(&ui, &view, &playlist, &entry, change);
            }
        });
        row.add_suffix(&button);
    };
    edit(
        icons::MOVE_UP,
        "Move up",
        index > 0,
        Edit::Move(index.saturating_sub(1)),
    );
    edit(
        icons::MOVE_DOWN,
        "Move down",
        index + 1 < tracks.len(),
        Edit::Move(index + 1),
    );
    edit(icons::REMOVE, "Remove from playlist", true, Edit::Remove);
    super::card::attach_menu(ui, &row, {
        let track = track.clone();
        move || Some(track.clone())
    });
    let (weak, tracks) = (ui.downgrade(), tracks.clone());
    row.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.play_music(tracks.to_vec(), index, false);
        }
    });
    row
}

#[derive(Clone, Copy)]
enum Edit {
    Move(usize),
    Remove,
}

fn edit_playlist(ui: &Ui, view: &Rc<PlaylistView>, playlist: &BaseItem, entry: &str, change: Edit) {
    let (client, id, entry) = (ui.client(), playlist.id.clone(), entry.to_string());
    let (ui, view, playlist) = (ui.clone(), view.clone(), playlist.clone());
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move {
            match change {
                Edit::Move(to) => client.move_in_playlist(&id, &entry, to).await,
                Edit::Remove => client.remove_from_playlist(&id, &[&entry]).await,
            }
        })
        .await;
        match result {
            Ok(()) => load(&ui, &view, &playlist),
            Err(e) => ui.report_error("Couldn't change the playlist", &e),
        }
    });
}

/// Asks which playlist (or a new one) gets `item`'s tracks.
pub fn add_dialog(ui: &Ui, item: &BaseItem) {
    let dialog = adw::AlertDialog::builder()
        .heading("Add to Playlist")
        .body(glib::markup_escape_text(&item.name))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.set_close_response("cancel");
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let name = adw::EntryRow::builder()
        .title("New playlist")
        .show_apply_button(true)
        .build();
    list.append(&name);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(360)
        .child(&list)
        .build();
    dialog.set_extra_child(Some(&scrolled));

    let add_to = {
        let (weak, item, dialog) = (ui.downgrade(), item.clone(), dialog.downgrade());
        move |target: Target| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            if let Some(ui) = weak.upgrade() {
                add(&ui, &item, target);
            }
        }
    };
    let add_to = Rc::new(add_to);
    {
        let add_to = add_to.clone();
        name.connect_apply(move |row| {
            let text = row.text().trim().to_string();
            if !text.is_empty() {
                add_to(Target::New(text));
            }
        });
    }

    let (client, user_id, weak_list) = (ui.client(), ui.user_id(), list.downgrade());
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let playlists = spawn_tokio(async move { client.playlists(&user_id).await }).await;
        let (Some(list), Some(ui)) = (weak_list.upgrade(), weak.upgrade()) else {
            return;
        };
        match playlists {
            Ok(playlists) => {
                for playlist in playlists {
                    let row = adw::ActionRow::builder()
                        .title(glib::markup_escape_text(&playlist.name))
                        .activatable(true)
                        .build();
                    row.add_prefix(&gtk::Image::from_icon_name(icons::PLAYLIST));
                    let (add_to, id) = (add_to.clone(), playlist.id.clone());
                    let name = playlist.name.clone();
                    row.connect_activated(move |_| {
                        add_to(Target::Existing(id.clone(), name.clone()))
                    });
                    list.append(&row);
                }
            }
            Err(e) => ui.report_error("Could not load playlists", &e),
        }
    });
    if let Some(window) = ui.widget().root().and_downcast::<gtk::Window>() {
        dialog.present(Some(&window));
    }
}

enum Target {
    Existing(String, String),
    New(String),
}

fn add(ui: &Ui, item: &BaseItem, target: Target) {
    let (client, user_id, item_id) = (ui.client(), ui.user_id(), item.id.clone());
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move {
            match target {
                Target::Existing(id, name) => client
                    .add_to_playlist(&user_id, &id, &[&item_id])
                    .await
                    .map(|()| name),
                Target::New(name) => client
                    .create_playlist(&name, &[&item_id])
                    .await
                    .map(|_| name),
            }
        })
        .await;
        match result {
            Ok(name) => {
                ui.toast(&format!("Added to {name}"));
                ui.data_changed();
            }
            Err(e) => ui.report_error("Couldn't add to the playlist", &e),
        }
    });
}
