//! Favourites split by kind (series, movies, episodes, music), as rows
//! whose titles open the full sortable grid of that kind.

use adw::prelude::*;
use gtk::glib;

use super::card::Shape;
use super::rows::{Click, More, row};
use super::{Ui, clear, loading};
use crate::emby::browse::ItemQuery;
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

const ROW_LIMIT: usize = 24;

/// One kind of favourite: heading, Emby item type, card shape.
pub type Section = (&'static str, &'static str, Shape);

pub const ALL: [Section; 6] = [
    ("Series", "Series", Shape::Poster),
    ("Movies", "Movie", Shape::Poster),
    ("Episodes", "Episode", Shape::Landscape),
    ("Albums", "MusicAlbum", Shape::Square),
    ("Songs", "Audio", Shape::Square),
    ("Artists", "MusicArtist", Shape::Person),
];

/// The favourite kinds a library of `collection_type` can hold.
pub fn sections_for(collection_type: Option<&str>) -> Vec<Section> {
    let kinds: &[&str] = match collection_type {
        Some("tvshows") => &["Series", "Episode"],
        Some("movies") => &["Movie"],
        Some("music") => &["MusicAlbum", "Audio", "MusicArtist"],
        _ => &[
            "Series",
            "Movie",
            "Episode",
            "MusicAlbum",
            "Audio",
            "MusicArtist",
        ],
    };
    ALL.into_iter()
        .filter(|(_, kind, _)| kinds.contains(kind))
        .collect()
}

fn query(parent_id: Option<&str>, kind: &'static str) -> ItemQuery {
    ItemQuery {
        parent_id: parent_id.map(str::to_string),
        include_types: Some(kind),
        recursive: true,
        filters: vec!["IsFavorite"],
        ..Default::default()
    }
}

/// Rows of favourites, optionally within one library.
pub fn view(ui: &Ui, parent_id: Option<String>, sections: Vec<Section>) -> gtk::Widget {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(12)
        .margin_bottom(24)
        .build();
    content.append(&loading());
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .vexpand(true)
        .build();

    let client = ui.client();
    let user_id = ui.user_id();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let fetch_parent = parent_id.clone();
        let kinds: Vec<&'static str> = sections.iter().map(|(_, kind, _)| *kind).collect();
        let results = spawn_tokio(async move {
            let mut found = Vec::new();
            for kind in kinds {
                let mut query = query(fetch_parent.as_deref(), kind);
                query.limit = ROW_LIMIT;
                found.push(client.items(&user_id, &query).await.map(|page| page.items));
            }
            found
        })
        .await;
        let Some(ui) = weak.upgrade() else { return };
        clear(&content);
        let mut any = false;
        for ((title, kind, shape), items) in sections.into_iter().zip(results) {
            let items: Vec<BaseItem> = match items {
                Ok(items) => items,
                Err(e) => {
                    tracing::warn!("favourite {kind} failed: {e:#}");
                    continue;
                }
            };
            if items.is_empty() {
                continue;
            }
            any = true;
            let more = More::Grid(title, Box::new(query(parent_id.as_deref(), kind)));
            content.append(&row(&ui, title, &items, shape, Click::Open, more));
        }
        if !any {
            content.append(
                &adw::StatusPage::builder()
                    .icon_name(crate::ui::icons::NOT_FAVORITE)
                    .title("No favourites yet")
                    .description("Use the star on any title, or its menu, to add it here")
                    .vexpand(true)
                    .build(),
            );
        }
    });
    scrolled.upcast()
}

/// The Favorites page reached from Home.
pub fn page(ui: &Ui) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    super::add_home_button(&header);
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&view(ui, None, ALL.to_vec())));
    adw::NavigationPage::new(&toolbar, "Favorites")
}

#[cfg(test)]
mod tests {
    use super::sections_for;

    #[test]
    fn libraries_only_offer_their_own_kinds() {
        let kinds = |c| {
            sections_for(c)
                .into_iter()
                .map(|(_, k, _)| k)
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(Some("tvshows")), ["Series", "Episode"]);
        assert_eq!(kinds(Some("movies")), ["Movie"]);
        assert_eq!(kinds(Some("music")), ["MusicAlbum", "Audio", "MusicArtist"]);
        assert_eq!(kinds(None).len(), 6);
    }
}
