//! A library's page. Movie and TV libraries get Emby-web-style tabs;
//! anything else (folders, collections) is one sortable grid.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::card::Shape;
use super::library::{self, PAGE_SIZE};
use super::rows::{Click, More, row};
use super::{Ui, WeakUi, clear, loading};
use crate::emby::browse::ItemQuery;
use crate::emby::models::{BaseItem, Recommendation};
use crate::runtime::spawn_tokio;

const SUGGESTION_ROW_LIMIT: usize = 20;
const RECOMMENDATION_ROWS: usize = 6;
const RECOMMENDATION_ITEMS: usize = 12;

/// Sort choices: label, Emby `SortBy`, and whether it defaults to
/// descending (newest/highest first).
const SORTS: [(&str, &str, bool); 7] = [
    ("Name", "SortName", false),
    ("Date Added", "DateCreated", true),
    ("Last Episode Added", LAST_EPISODE_ADDED, true),
    ("Release Date", "PremiereDate", true),
    ("Rating", "CommunityRating", true),
    ("Runtime", "Runtime", false),
    ("Random", "Random", false),
];

/// Series only: when a series last got a new episode.
const LAST_EPISODE_ADDED: &str = "DateLastContentAdded";

/// Filter choices: label and Emby `Filters` value.
const FILTERS: [(&str, Option<&str>); 5] = [
    ("All", None),
    ("Unplayed", Some("IsUnplayed")),
    ("Played", Some("IsPlayed")),
    ("In Progress", Some("IsResumable")),
    ("Favorites", Some("IsFavorite")),
];

/// Decade choices: label and first year (0 is everything before 1970).
const DECADES: [(&str, u16); 7] = [
    ("2020s", 2020),
    ("2010s", 2010),
    ("2000s", 2000),
    ("1990s", 1990),
    ("1980s", 1980),
    ("1970s", 1970),
    ("Older", 0),
];

/// "Added" choices: label and how many days back.
const ADDED: [(&str, u32); 3] = [
    ("Last 7 days", 7),
    ("Last 30 days", 30),
    ("Last 90 days", 90),
];

/// The grid filters beyond the status dropdown, set from the Filters popover
/// (Movies and Shows grids only).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Extra {
    /// First year of the decade, as in `DECADES`.
    decade: Option<u16>,
    /// `Some(true)` for HD, `Some(false)` for SD.
    hd: Option<bool>,
    subtitles: bool,
    rating: Option<String>,
    /// Added within this many days, as in `ADDED`.
    added_days: Option<u32>,
}

impl Extra {
    /// How many filters are on.
    fn active(&self) -> usize {
        usize::from(self.decade.is_some())
            + usize::from(self.hd.is_some())
            + usize::from(self.subtitles)
            + usize::from(self.rating.is_some())
            + usize::from(self.added_days.is_some())
    }

    /// The saved form, `v1;decade=2010;res=hd;subs=1;rating=PG-13;added=30`
    /// (None when nothing is on). A rating with a `;` can't be saved, and
    /// real ones have none.
    fn encode(&self) -> Option<String> {
        let mut parts = vec!["v1".to_string()];
        if let Some(decade) = self.decade {
            parts.push(format!("decade={decade}"));
        }
        if let Some(hd) = self.hd {
            parts.push(format!("res={}", if hd { "hd" } else { "sd" }));
        }
        if self.subtitles {
            parts.push("subs=1".to_string());
        }
        if let Some(rating) = self.rating.as_ref().filter(|r| !r.contains(';')) {
            parts.push(format!("rating={rating}"));
        }
        if let Some(days) = self.added_days {
            parts.push(format!("added={days}"));
        }
        (parts.len() > 1).then(|| parts.join(";"))
    }

    /// Reads `encode`'s form; anything unknown or invalid is left off.
    fn decode(text: &str) -> Self {
        let mut parts = text.split(';');
        let mut extra = Self::default();
        if parts.next() != Some("v1") {
            return extra;
        }
        for (key, value) in parts.filter_map(|part| part.split_once('=')) {
            match key {
                "decade" => {
                    extra.decade = value
                        .parse()
                        .ok()
                        .filter(|year| DECADES.iter().any(|(_, y)| y == year));
                }
                "res" => {
                    extra.hd = match value {
                        "hd" => Some(true),
                        "sd" => Some(false),
                        _ => None,
                    };
                }
                "subs" => extra.subtitles = value == "1",
                "rating" => extra.rating = Some(value.to_string()).filter(|r| !r.is_empty()),
                "added" => {
                    extra.added_days = value
                        .parse()
                        .ok()
                        .filter(|days| ADDED.iter().any(|(_, d)| d == days));
                }
                _ => {}
            }
        }
        extra
    }

    /// Every year of the decade starting `decade` (up to `this_year`, for
    /// the current one), comma-separated for Emby's `Years`; 0 is 1900-1969.
    fn years(decade: u16, this_year: i64) -> String {
        let (first, last) = match decade {
            0 => (1900, 1969),
            d => (i64::from(d), (i64::from(d) + 9).min(this_year)),
        };
        (first..=last)
            .map(|year| year.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Puts what the server can filter on straight into `query`. Resolution
    /// and subtitles only mean something for movies: a series has no streams
    /// of its own, so those go by episodes (see `IdRestriction`).
    fn apply(&self, query: &mut ItemQuery, series: bool, this_year: i64) {
        query.years = self.decade.map(|decade| Self::years(decade, this_year));
        query.official_ratings = self.rating.clone();
        if !series {
            query.is_hd = self.hd;
            query.has_subtitles = self.subtitles.then_some(true);
        }
    }
}

/// UTC date and time of Unix `secs`: year, month, day, hour, minute, second.
fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 0000-03-01 in 400-year eras (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        month as u32,
        day as u32,
        (rest / 3600) as u32,
        (rest % 3600 / 60) as u32,
        (rest % 60) as u32,
    )
}

/// `secs` as `YYYY-MM-DDTHH:MM:SS` (UTC), which sorts as text like Emby's
/// `DateCreated`.
fn iso(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// What the grid is cut down to by a set of item ids the server can't
/// filter for us (see `restricted_ids`); the sets found are intersected.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IdRestriction {
    series: bool,
    parent: Option<String>,
    /// Series started and not finished.
    in_progress: bool,
    /// Series with an episode that is HD / has subtitles.
    episodes: Option<(Option<bool>, Option<bool>)>,
    /// Added on or after this `iso` time.
    added_since: Option<String>,
}

impl IdRestriction {
    fn new(
        parent: Option<String>,
        series: bool,
        in_progress: bool,
        extra: &Extra,
        now: i64,
    ) -> Self {
        let wants_episodes = series && (extra.hd.is_some() || extra.subtitles);
        IdRestriction {
            series,
            parent,
            in_progress,
            episodes: wants_episodes.then_some((extra.hd, extra.subtitles.then_some(true))),
            added_since: extra
                .added_days
                .map(|days| iso(now - i64::from(days) * 86_400)),
        }
    }

    fn is_none(&self) -> bool {
        !self.in_progress && self.episodes.is_none() && self.added_since.is_none()
    }
}

/// The ids every part of `restriction` allows.
async fn restricted_ids(
    client: &crate::emby::EmbyClient,
    user_id: &str,
    restriction: &IdRestriction,
) -> anyhow::Result<Vec<String>> {
    let parent = restriction.parent.as_deref();
    let mut sets = Vec::new();
    if restriction.in_progress {
        sets.push(client.started_series_ids(user_id, parent).await?);
    }
    if let Some((hd, subtitles)) = restriction.episodes {
        sets.push(
            client
                .series_ids_with_episodes(user_id, parent, hd, subtitles)
                .await?,
        );
    }
    if let Some(cutoff) = &restriction.added_since {
        sets.push(
            client
                .added_since_ids(user_id, parent, restriction.series, cutoff)
                .await?,
        );
    }
    Ok(intersect(sets))
}

/// The ids in every one of `sets`, in the first one's order.
fn intersect(mut sets: Vec<Vec<String>>) -> Vec<String> {
    if sets.is_empty() {
        return Vec::new();
    }
    let first = sets.remove(0);
    let rest: Vec<HashSet<String>> = sets.into_iter().map(|s| s.into_iter().collect()).collect();
    first
        .into_iter()
        .filter(|id| rest.iter().all(|set| set.contains(id)))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Movies,
    Shows,
}

impl Kind {
    fn of(folder: &BaseItem) -> Option<Self> {
        match folder.collection_type.as_deref() {
            Some("movies") => Some(Kind::Movies),
            Some("tvshows") => Some(Kind::Shows),
            _ => None,
        }
    }

    fn item_type(self) -> &'static str {
        match self {
            Kind::Movies => "Movie",
            Kind::Shows => "Series",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Chips {
    Genres,
    Tags,
}

pub fn page(ui: &Ui, folder: &BaseItem) -> adw::NavigationPage {
    if folder.collection_type.as_deref() == Some("music") {
        return music(ui, folder);
    }
    match Kind::of(folder) {
        Some(kind) => tabbed(ui, folder, kind),
        None => plain(ui, folder),
    }
}

/// Everything a person appears in.
pub fn person(ui: &Ui, person: &BaseItem) -> adw::NavigationPage {
    let query = ItemQuery {
        person_id: Some(person.id.clone()),
        include_types: Some("Movie,Series"),
        recursive: true,
        ..Default::default()
    };
    toolbar_page(
        &person.name,
        &adw::HeaderBar::new(),
        &sortable_grid(ui, query),
    )
}

/// The movies and shows in a genre or tag, across all libraries.
pub fn category(ui: &Ui, category: &BaseItem) -> adw::NavigationPage {
    grid_page(ui, &category.name, category_query(category))
}

/// The movies and shows in a genre or tag (by its `item_type`).
pub fn category_query(category: &BaseItem) -> ItemQuery {
    let mut query = ItemQuery {
        include_types: Some("Movie,Series"),
        recursive: true,
        ..Default::default()
    };
    if category.item_type == "Tag" {
        query.tag_id = Some(category.id.clone());
    } else {
        query.genre_id = Some(category.id.clone());
    }
    query
}

fn plain(ui: &Ui, folder: &BaseItem) -> adw::NavigationPage {
    let include_types = match folder.collection_type.as_deref() {
        Some("boxsets") => Some("BoxSet"),
        Some("musicvideos") => Some("MusicVideo"),
        _ => None,
    };
    let query = ItemQuery {
        parent_id: Some(folder.id.clone()),
        include_types,
        recursive: include_types.is_some(),
        ..Default::default()
    };
    toolbar_page(
        &folder.name,
        &adw::HeaderBar::new(),
        &sortable_grid(ui, query),
    )
}

/// A titled page holding a sortable grid of `query`.
pub fn grid_page(ui: &Ui, title: &str, query: ItemQuery) -> adw::NavigationPage {
    toolbar_page(title, &adw::HeaderBar::new(), &sortable_grid(ui, query))
}

/// A page whose content scrolls by itself (grids must sit directly in
/// their ScrolledWindow to stay virtualized).
fn toolbar_page(
    title: &str,
    header: &adw::HeaderBar,
    content: &impl IsA<gtk::Widget>,
) -> adw::NavigationPage {
    super::add_home_button(header);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(header);
    toolbar.set_content(Some(content));
    adw::NavigationPage::new(&toolbar, title)
}

type TabBuilder = Box<dyn FnOnce(&Ui) -> gtk::Widget>;
/// Name, title, icon, and how to build it.
type Tab = (&'static str, &'static str, &'static str, TabBuilder);

fn tabbed(ui: &Ui, folder: &BaseItem, kind: Kind) -> adw::NavigationPage {
    let item_type = kind.item_type();
    let library_id = folder.id.clone();
    let in_library = |include_types: Option<&'static str>, recursive: bool| ItemQuery {
        parent_id: Some(library_id.clone()),
        include_types,
        recursive,
        ..Default::default()
    };

    let mut tabs: Vec<Tab> = Vec::new();
    let items = in_library(Some(item_type), true);
    tabs.push((
        "items",
        if kind == Kind::Movies {
            "Movies"
        } else {
            "Shows"
        },
        if kind == Kind::Movies {
            crate::ui::icons::MOVIES
        } else {
            crate::ui::icons::SHOWS
        },
        Box::new(move |ui| sortable_grid(ui, items).upcast()),
    ));
    let suggestions_folder = folder.clone();
    tabs.push((
        "suggestions",
        "Suggestions",
        crate::ui::icons::SUGGESTIONS,
        Box::new(move |ui| suggestions(ui, &suggestions_folder, kind)),
    ));
    if kind == Kind::Shows {
        let upcoming_library = library_id.clone();
        tabs.push((
            "upcoming",
            "Upcoming",
            crate::ui::icons::UPCOMING,
            Box::new(move |ui| upcoming(ui, &upcoming_library)),
        ));
    }
    // Shown only once the library turns out to have any (see `tab_page`).
    let collections = in_library(Some("BoxSet"), true);
    tabs.push((
        COLLECTIONS_TAB,
        "Collections",
        crate::ui::icons::COLLECTIONS,
        Box::new(move |ui| sortable_grid(ui, collections).upcast()),
    ));
    for (name, title, icon, chips_kind) in [
        ("genres", "Genres", crate::ui::icons::GENRES, Chips::Genres),
        ("tags", "Tags", crate::ui::icons::TAGS, Chips::Tags),
    ] {
        let library_id = library_id.clone();
        tabs.push((
            name,
            title,
            icon,
            Box::new(move |ui| chips(ui, &library_id, item_type, chips_kind)),
        ));
    }
    let favorites_library = folder.id.clone();
    let collection = folder.collection_type.clone();
    tabs.push((
        "favorites",
        "Favorites",
        crate::ui::icons::FAVORITE,
        Box::new(move |ui| {
            super::favorites::view(
                ui,
                Some(favorites_library),
                super::favorites::sections_for(collection.as_deref()),
            )
        }),
    ));
    let folders = in_library(None, false);
    tabs.push((
        "folders",
        "Folders",
        crate::ui::icons::FOLDER,
        Box::new(move |ui| sortable_grid(ui, folders).upcast()),
    ));

    tab_page(ui, folder, tabs)
}

const COLLECTIONS_TAB: &str = "collections";

/// Hides the Collections tab until Emby says the library has some.
fn hide_unless_collections(ui: &Ui, tab: &adw::ViewStackPage, library_id: &str) {
    tab.set_visible(false);
    let query = ItemQuery {
        parent_id: Some(library_id.to_string()),
        include_types: Some("BoxSet"),
        recursive: true,
        limit: 1,
        ..Default::default()
    };
    let client = ui.client();
    let user_id = ui.user_id();
    let tab = tab.downgrade();
    glib::spawn_future_local(async move {
        let found = spawn_tokio(async move { client.items(&user_id, &query).await }).await;
        if let (Ok(result), Some(tab)) = (found, tab.upgrade()) {
            tab.set_visible(!result.items.is_empty());
        }
    });
}

/// A library page with these tabs in its header.
fn tab_page(ui: &Ui, folder: &BaseItem, tabs: Vec<Tab>) -> adw::NavigationPage {
    // Tabs are built the first time they're shown, so opening a library
    // only costs the requests of the tab you're looking at.
    let stack = adw::ViewStack::new();
    let pending: Rc<RefCell<HashMap<String, TabBuilder>>> = Rc::default();
    for (name, title, icon, build) in tabs {
        stack.add_titled_with_icon(&adw::Bin::new(), Some(name), title, icon);
        pending.borrow_mut().insert(name.to_string(), build);
    }
    let build_visible = {
        let weak = ui.downgrade();
        move |stack: &adw::ViewStack| {
            let (Some(ui), Some(name)) = (weak.upgrade(), stack.visible_child_name()) else {
                return;
            };
            let Some(build) = pending.borrow_mut().remove(name.as_str()) else {
                return;
            };
            if let Some(bin) = stack.child_by_name(&name).and_downcast::<adw::Bin>() {
                bin.set_child(Some(&build(&ui)));
            }
        }
    };
    build_visible(&stack);
    stack.connect_visible_child_name_notify(build_visible);
    if let Some(tab) = stack.child_by_name(COLLECTIONS_TAB) {
        hide_unless_collections(ui, &stack.page(&tab), &folder.id);
    }

    let header = adw::HeaderBar::builder()
        .title_widget(
            &adw::ViewSwitcher::builder()
                .stack(&stack)
                .policy(adw::ViewSwitcherPolicy::Wide)
                .build(),
        )
        .build();
    let page = toolbar_page(&folder.name, &header, &stack);
    super::set_tab_stepper(&page, move |forward| step_stack(&stack, forward));
    page
}

/// A music library: albums, artists, songs, what's new, favourites, folders.
fn music(ui: &Ui, folder: &BaseItem) -> adw::NavigationPage {
    let library_id = folder.id.clone();
    let in_library = |include_types: &'static str| ItemQuery {
        parent_id: Some(library_id.clone()),
        include_types: Some(include_types),
        recursive: true,
        ..Default::default()
    };
    let albums = in_library("MusicAlbum");
    let songs = in_library("Audio");
    let folders = ItemQuery {
        parent_id: Some(library_id.clone()),
        ..Default::default()
    };
    let artists_library = library_id.clone();
    let latest_library = library_id.clone();
    let favorites_library = library_id.clone();
    let tabs: Vec<Tab> = vec![
        (
            "albums",
            "Albums",
            crate::ui::icons::ALBUMS,
            Box::new(move |ui| sortable_grid_of(ui, albums, Shape::Square).upcast()),
        ),
        (
            "artists",
            "Artists",
            crate::ui::icons::ARTISTS,
            Box::new(move |ui| artists_grid(ui, &artists_library)),
        ),
        (
            "songs",
            "Songs",
            crate::ui::icons::SONGS,
            Box::new(move |ui| sortable_grid_of(ui, songs, Shape::Square).upcast()),
        ),
        (
            "playlists",
            "Playlists",
            crate::ui::icons::PLAYLIST,
            Box::new(super::playlists::grid),
        ),
        (
            "latest",
            "Latest",
            crate::ui::icons::SUGGESTIONS,
            Box::new(move |ui| latest_albums(ui, &latest_library)),
        ),
        (
            "favorites",
            "Favorites",
            crate::ui::icons::FAVORITE,
            Box::new(move |ui| {
                super::favorites::view(
                    ui,
                    Some(favorites_library),
                    super::favorites::sections_for(Some("music")),
                )
            }),
        ),
        (
            "folders",
            "Folders",
            crate::ui::icons::FOLDER,
            Box::new(move |ui| sortable_grid_of(ui, folders, Shape::Square).upcast()),
        ),
    ];
    tab_page(ui, folder, tabs)
}

/// Album artists as round cards; each opens their albums.
fn artists_grid(ui: &Ui, library_id: &str) -> gtk::Widget {
    let (grid, store) = library::grid(ui, Shape::Person);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .vexpand(true)
        .build();
    let client = ui.client();
    let user_id = ui.user_id();
    let library_id = library_id.to_string();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let result =
            spawn_tokio(async move { client.album_artists(&user_id, &library_id).await }).await;
        match result {
            Ok(artists) => {
                let objects: Vec<_> = artists.into_iter().map(glib::BoxedAnyObject::new).collect();
                store.extend_from_slice(&objects);
            }
            Err(e) => {
                if let Some(ui) = weak.upgrade() {
                    ui.report_error("Could not load artists", &e);
                }
            }
        }
    });
    scrolled.upcast()
}

/// Recently added albums.
fn latest_albums(ui: &Ui, library_id: &str) -> gtk::Widget {
    let library_id = library_id.to_string();
    let (grid, store) = library::grid(ui, Shape::Square);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .vexpand(true)
        .build();
    let client = ui.client();
    let user_id = ui.user_id();
    glib::spawn_future_local(async move {
        if let Ok(albums) =
            spawn_tokio(async move { client.latest(&user_id, &library_id, 60).await }).await
        {
            let objects: Vec<_> = albums.into_iter().map(glib::BoxedAnyObject::new).collect();
            store.extend_from_slice(&objects);
        }
    });
    scrolled.upcast()
}

/// An artist's albums.
pub fn artist(ui: &Ui, artist: &BaseItem) -> adw::NavigationPage {
    let query = ItemQuery {
        artist_id: Some(artist.id.clone()),
        include_types: Some("MusicAlbum"),
        recursive: true,
        sort_by: Some("ProductionYear"),
        descending: true,
        ..Default::default()
    };
    toolbar_page(
        &artist.name,
        &adw::HeaderBar::new(),
        &sortable_grid_of(ui, query, Shape::Square),
    )
}

/// Shows the next/previous tab of `stack` (controller bumpers).
/// Steps to the previous/next tab, skipping hidden ones.
pub(super) fn step_stack(stack: &adw::ViewStack, forward: bool) {
    let pages: Vec<adw::ViewStackPage> = {
        let model = stack.pages();
        (0..model.n_items())
            .filter_map(|i| model.item(i).and_downcast::<adw::ViewStackPage>())
            .filter(|page| page.is_visible())
            .collect()
    };
    let Some(current) = pages
        .iter()
        .position(|page| Some(page.child()) == stack.visible_child())
    else {
        return;
    };
    let next = if forward {
        (current + 1).min(pages.len().saturating_sub(1))
    } else {
        current.saturating_sub(1)
    };
    stack.set_visible_child(&pages[next].child());
}

/// A poster grid of `base` with a count, filter, sort and order bar above.
fn sortable_grid(ui: &Ui, base: ItemQuery) -> gtk::Box {
    sortable_grid_of(ui, base, Shape::Poster)
}

fn sortable_grid_of(ui: &Ui, mut base: ItemQuery, shape: Shape) -> gtk::Box {
    let (grid, store) = library::grid(ui, shape);
    // The A-Z strip files items by their sort name.
    base.extra_fields = base.extra_fields.or(Some("SortName"));
    let count = gtk::Label::builder()
        .margin_end(6)
        .css_classes(["dim-label"])
        .build();
    // TV's "In Progress" is worked out from the episodes (see
    // `reload_grid`); Preferences can leave it out.
    let series = base.include_types.is_some_and(|t| t.contains("Series"));
    let series_in_progress = crate::config::Settings::load()
        .unwrap_or_default()
        .home
        .series_in_progress;
    let filters: Rc<Vec<(&'static str, Option<&'static str>)>> = Rc::new(
        FILTERS
            .into_iter()
            .filter(|(_, f)| series_in_progress || !series || *f != Some("IsResumable"))
            .collect(),
    );
    let labels: Vec<&str> = filters.iter().map(|(label, _)| *label).collect();
    let filter = gtk::DropDown::from_strings(&labels);
    filter.set_tooltip_text(Some("Filter"));
    let only_series = base.include_types == Some("Series");
    let sorts: Rc<Vec<(&'static str, &'static str, bool)>> = Rc::new(
        SORTS
            .into_iter()
            .filter(|(_, s, _)| only_series || *s != LAST_EPISODE_ADDED)
            .collect(),
    );
    let labels: Vec<&str> = sorts.iter().map(|(label, _, _)| *label).collect();
    let sort = gtk::DropDown::from_strings(&labels);
    sort.set_tooltip_text(Some("Sort by"));
    let descending = gtk::ToggleButton::builder()
        .icon_name(crate::ui::icons::SORT_ASCENDING)
        .tooltip_text("Sort order")
        .build();

    // Count, filter and sort together in the middle, over the centred grid.
    let bar = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .margin_start(18)
        .margin_end(18)
        .margin_top(6)
        .css_classes([library::GRID_BAR_CLASS])
        .build();
    bar.append(&count);
    bar.append(&filter);
    bar.append(&sort);
    bar.append(&descending);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .vexpand(true)
        .build();
    // The jump-to-letter strip beside the grid, for name order.
    let strip = library::az_strip(&grid, &store);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&bar);
    root.append(&library::grid_body(&scrolled, &strip));

    // More filters, on Movies and Shows grids only.
    let item_type = base
        .include_types
        .filter(|t| matches!(*t, "Movie" | "Series"));
    let library_id = base.parent_id.clone();
    let extra = Rc::new(RefCell::new(Extra::default()));

    // The last sort/filter used here, before any handler is connected.
    let key = view_key(&base);
    if let Some(prefs) = crate::db::Db::open_default()
        .and_then(|db| db.get_view_prefs(&crate::account::scope(), &key))
        .unwrap_or_else(|e| {
            tracing::warn!("couldn't read view settings: {e:#}");
            None
        })
    {
        if let Some(index) = sorts.iter().position(|(_, s, _)| *s == prefs.sort_by) {
            sort.set_selected(index as u32);
        }
        if let Some(index) = filters
            .iter()
            .position(|(_, f)| *f == prefs.filter.as_deref())
        {
            filter.set_selected(index as u32);
        }
        descending.set_active(prefs.descending);
        descending.set_icon_name(if prefs.descending {
            crate::ui::icons::SORT_DESCENDING
        } else {
            crate::ui::icons::SORT_ASCENDING
        });
        if item_type.is_some() {
            *extra.borrow_mut() = prefs
                .extra
                .as_deref()
                .map(Extra::decode)
                .unwrap_or_default();
        }
    }
    let opened = Rc::new(Cell::new(true));

    // Each reload bumps this; pages from an older query are dropped.
    let generation = Rc::new(Cell::new(0u64));
    let reload = Rc::new(glib::clone!(
        #[weak]
        store,
        #[weak]
        count,
        #[weak]
        filter,
        #[weak]
        sort,
        #[weak]
        descending,
        #[weak]
        strip,
        #[strong]
        generation,
        #[strong]
        sorts,
        #[strong]
        extra,
        #[strong(rename_to = weak_ui)]
        ui.downgrade(),
        move || {
            let choice = Choice {
                sort: sorts[sort.selected() as usize].1,
                descending: descending.is_active(),
                filter: filters
                    .get(filter.selected() as usize)
                    .and_then(|(_, f)| *f),
                extra: extra.borrow().clone(),
            };
            // Letters only mean something in name order.
            strip.set_visible(choice.sort == "SortName");
            // Remembered for next time (not the first load: nothing changed).
            if !opened.replace(false) {
                remember(&key, &choice);
            }
            reload_grid(&weak_ui, &store, &count, &base, choice, &generation)
        }
    ));

    descending.connect_toggled({
        let reload = reload.clone();
        move |button| {
            button.set_icon_name(if button.is_active() {
                crate::ui::icons::SORT_DESCENDING
            } else {
                crate::ui::icons::SORT_ASCENDING
            });
            reload();
        }
    });
    sort.connect_selected_notify(glib::clone!(
        #[weak]
        descending,
        #[strong]
        reload,
        move |sort| {
            // Each sort has a natural direction; toggling it reloads too.
            let natural = sorts[sort.selected() as usize].2;
            if descending.is_active() != natural {
                descending.set_active(natural);
            } else {
                reload();
            }
        }
    ));
    filter.connect_selected_notify({
        let reload = reload.clone();
        move |_| reload()
    });
    if let Some(item_type) = item_type {
        let changed: Rc<dyn Fn()> = reload.clone();
        let more = extra_filters(ui, library_id, item_type, &extra, changed);
        bar.insert_child_after(&more, Some(&filter));
    }
    reload();
    root
}

/// What the Filters popover and its button share.
struct FilterState {
    extra: Rc<RefCell<Extra>>,
    /// Set while rows are being set from `extra` (not by the user).
    syncing: Cell<bool>,
    button: glib::WeakRef<gtk::MenuButton>,
    badge: glib::WeakRef<gtk::Label>,
    changed: Rc<dyn Fn()>,
}

impl FilterState {
    /// The button's count and accent, for the filters on.
    fn refresh(&self) {
        let active = self.extra.borrow().active();
        if let Some(badge) = self.badge.upgrade() {
            badge.set_label(&active.to_string());
            badge.set_visible(active > 0);
        }
        if let Some(button) = self.button.upgrade() {
            if active > 0 {
                button.add_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
            }
        }
    }

    /// A change the user made: apply it, then reload the grid.
    fn edit(&self, set: impl FnOnce(&mut Extra)) {
        set(&mut self.extra.borrow_mut());
        self.refresh();
        (self.changed)();
    }
}

/// A label with a drop-down of `options` (the first is "Any").
fn filter_row(label: &str, options: &[&str]) -> (gtk::Box, gtk::DropDown) {
    let row = gtk::Box::builder().spacing(18).build();
    row.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    let dropdown = gtk::DropDown::from_strings(options);
    row.append(&dropdown);
    (row, dropdown)
}

/// Runs `set` on the filters when the user picks something in `dropdown`.
fn wire_row(
    dropdown: &gtk::DropDown,
    state: &Rc<FilterState>,
    set: impl Fn(&mut Extra, u32) + 'static,
) {
    let state = state.clone();
    dropdown.connect_selected_notify(move |dropdown| {
        if !state.syncing.get() {
            let picked = dropdown.selected();
            state.edit(|extra| set(extra, picked));
        }
    });
}

/// The bar's Filters button: a popover with a row per extra filter. Each
/// change reloads the grid at once and the popover stays open.
fn extra_filters(
    ui: &Ui,
    library_id: Option<String>,
    item_type: &'static str,
    extra: &Rc<RefCell<Extra>>,
    changed: Rc<dyn Fn()>,
) -> gtk::MenuButton {
    let badge = gtk::Label::new(None);
    let face = gtk::Box::builder().spacing(4).build();
    face.append(&gtk::Image::from_icon_name(crate::ui::icons::FILTER));
    face.append(&badge);
    let button = gtk::MenuButton::builder()
        .child(&face)
        .tooltip_text("More filters")
        .build();
    let state = Rc::new(FilterState {
        extra: extra.clone(),
        syncing: Cell::new(false),
        button: button.downgrade(),
        badge: badge.downgrade(),
        changed,
    });

    let decades: Vec<&str> = std::iter::once("Any")
        .chain(DECADES.iter().map(|(label, _)| *label))
        .collect();
    let (decade_row, decade) = filter_row("Decade", &decades);
    let (res_row, res) = filter_row("Resolution", &["Any", "HD", "SD"]);
    let (subs_row, subs) = filter_row("Subtitles", &["Any", "With subtitles"]);
    let (rating_row, rating) = filter_row("Rating", &["Any"]);
    // Only once the library's ratings are known.
    rating_row.set_visible(false);
    let added: Vec<&str> = std::iter::once("Any")
        .chain(ADDED.iter().map(|(label, _)| *label))
        .collect();
    let (added_row, added) = filter_row("Added", &added);

    // Index 0 is "Any"; the rest follow their table.
    wire_row(&decade, &state, |extra, picked| {
        extra.decade = picked
            .checked_sub(1)
            .and_then(|i| DECADES.get(i as usize))
            .map(|(_, year)| *year);
    });
    wire_row(&res, &state, |extra, picked| {
        extra.hd = match picked {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        };
    });
    wire_row(&subs, &state, |extra, picked| extra.subtitles = picked == 1);
    wire_row(&added, &state, |extra, picked| {
        extra.added_days = picked
            .checked_sub(1)
            .and_then(|i| ADDED.get(i as usize))
            .map(|(_, days)| *days);
    });
    // The rating names are only known once they're fetched.
    let ratings: Rc<RefCell<Vec<String>>> = Rc::default();
    wire_row(&rating, &state, {
        let ratings = ratings.clone();
        move |extra, picked| {
            extra.rating = picked
                .checked_sub(1)
                .and_then(|i| ratings.borrow().get(i as usize).cloned());
        }
    });

    // Rows to match `extra` (saved filters, or cleared).
    let sync = Rc::new(glib::clone!(
        #[weak]
        decade,
        #[weak]
        res,
        #[weak]
        subs,
        #[weak]
        rating,
        #[weak]
        added,
        #[strong]
        state,
        #[strong]
        ratings,
        move || {
            state.syncing.set(true);
            let extra = state.extra.borrow().clone();
            decade.set_selected(
                DECADES
                    .iter()
                    .position(|(_, year)| Some(*year) == extra.decade)
                    .map_or(0, |i| i as u32 + 1),
            );
            res.set_selected(match extra.hd {
                Some(true) => 1,
                Some(false) => 2,
                None => 0,
            });
            subs.set_selected(u32::from(extra.subtitles));
            rating.set_selected(
                ratings
                    .borrow()
                    .iter()
                    .position(|r| Some(r) == extra.rating.as_ref())
                    .map_or(0, |i| i as u32 + 1),
            );
            added.set_selected(
                ADDED
                    .iter()
                    .position(|(_, days)| Some(*days) == extra.added_days)
                    .map_or(0, |i| i as u32 + 1),
            );
            state.syncing.set(false);
            state.refresh();
        }
    ));
    sync();

    let clear = gtk::Button::with_label("Clear filters");
    clear.connect_clicked({
        let (state, sync) = (state.clone(), sync.clone());
        move |_| {
            if state.extra.borrow().active() > 0 {
                *state.extra.borrow_mut() = Extra::default();
                sync();
                (state.changed)();
            }
        }
    });

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    for row in [&decade_row, &res_row, &subs_row, &rating_row, &added_row] {
        content.append(row);
    }
    content.append(&clear);
    button.set_popover(Some(&gtk::Popover::builder().child(&content).build()));

    // This library's ratings, for the Rating row; no row without any.
    let (client, user_id) = (ui.client(), ui.user_id());
    let (weak_row, weak_rating) = (rating_row.downgrade(), rating.downgrade());
    glib::spawn_future_local(async move {
        let found = spawn_tokio(async move {
            client
                .official_ratings(&user_id, library_id.as_deref(), item_type)
                .await
        })
        .await;
        // Failed: keep a saved rating (the count on the button still shows
        // it, and Clear removes it) rather than lose it to a blip.
        let found = match found {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!("couldn't load ratings: {e:#}");
                return;
            }
        };
        let (Some(row), Some(rating)) = (weak_row.upgrade(), weak_rating.upgrade()) else {
            return;
        };
        let saved = state.extra.borrow().rating.clone();
        if !found.is_empty() {
            let options: Vec<&str> = std::iter::once("Any")
                .chain(found.iter().map(String::as_str))
                .collect();
            state.syncing.set(true);
            rating.set_model(Some(&gtk::StringList::new(&options)));
            state.syncing.set(false);
            *ratings.borrow_mut() = found;
            row.set_visible(true);
        }
        // A saved rating this library doesn't have (any more) would filter
        // with no way to see it: drop it.
        if saved.is_some_and(|saved| !ratings.borrow().contains(&saved)) {
            state.edit(|extra| extra.rating = None);
        }
        sync();
    });
    button
}

struct Choice {
    sort: &'static str,
    descending: bool,
    filter: Option<&'static str>,
    extra: Extra,
}

/// Which grid a remembered sort/filter belongs to: the library (or folder)
/// and the kind of items. Genre, tag and person views in a library share it.
fn view_key(query: &ItemQuery) -> String {
    format!(
        "{}|{}",
        query.parent_id.as_deref().unwrap_or("all"),
        query.include_types.unwrap_or("any")
    )
}

fn remember(key: &str, choice: &Choice) {
    let prefs = crate::db::ViewPrefs {
        sort_by: choice.sort.to_string(),
        descending: choice.descending,
        filter: choice.filter.map(str::to_string),
        extra: choice.extra.encode(),
    };
    if let Err(e) = crate::db::Db::open_default()
        .and_then(|db| db.set_view_prefs(&crate::account::scope(), key, &prefs))
    {
        tracing::warn!("couldn't save view settings: {e:#}");
    }
}

/// Most ids asked for in one request; a longer `Ids=` list makes the
/// address too long for the server.
const MAX_IDS: usize = 100;

fn reload_grid(
    ui: &WeakUi,
    store: &gtk::gio::ListStore,
    count: &gtk::Label,
    base: &ItemQuery,
    choice: Choice,
    generation: &Rc<Cell<u64>>,
) {
    let Some(ui) = ui.upgrade() else { return };
    let current = generation.get() + 1;
    generation.set(current);
    store.remove_all();
    count.set_label("");

    let mut query = base.clone();
    query.sort_by = Some(choice.sort);
    query.descending = choice.descending;
    query.filters.extend(choice.filter);
    query.limit = PAGE_SIZE;
    // "In Progress" for series: Emby never calls a series resumable, so
    // it's the series with an episode watched or part-watched, minus the
    // ones watched to the end.
    let in_progress = choice.filter == Some("IsResumable")
        && base.include_types.is_some_and(|t| t.contains("Series"));
    if in_progress {
        query.filters.retain(|f| *f != "IsResumable");
        query.filters.push("IsUnplayed");
    }
    let now = unix_now();
    let series = base.include_types == Some("Series");
    choice.extra.apply(&mut query, series, civil(now).0);
    let restriction = IdRestriction::new(
        base.parent_id.clone(),
        series || in_progress,
        in_progress,
        &choice.extra,
        now,
    );
    if restriction.is_none() {
        load_query(&ui, store, count, query, generation, current);
        return;
    }
    // Some filters Emby can't do itself: its ids come from elsewhere, and
    // the grid shows just those.
    let (client, user_id) = (ui.client(), ui.user_id());
    let (weak, store, count, generation) = (
        ui.downgrade(),
        store.clone(),
        count.downgrade(),
        generation.clone(),
    );
    glib::spawn_future_local(async move {
        let ids =
            spawn_tokio(async move { restricted_ids(&client, &user_id, &restriction).await }).await;
        if generation.get() != current {
            return;
        }
        let ids = match ids {
            Ok(ids) => ids,
            Err(e) => {
                // Like a failed page load, not an empty list.
                if let Some(ui) = weak.upgrade() {
                    ui.report_error("Could not load items", &e);
                }
                return;
            }
        };
        let Some(count) = count.upgrade() else { return };
        if ids.is_empty() {
            count.set_label("0 items");
            return;
        }
        if ids.len() <= MAX_IDS {
            query.ids = Some(ids.join(","));
            if let Some(ui) = weak.upgrade() {
                load_query(&ui, &store, &count, query, &generation, current);
            }
            return;
        }
        // Too many for one address: walk the library in the chosen
        // order and keep the allowed ones.
        let allowed: HashSet<String> = ids.into_iter().collect();
        let Some((client, user_id)) = weak.upgrade().map(|ui| (ui.client(), ui.user_id())) else {
            return;
        };
        let count = count.downgrade();
        let mut shown = 0;
        loop {
            let page = spawn_tokio({
                let (client, user_id, query) = (client.clone(), user_id.clone(), query.clone());
                async move { client.items(&user_id, &query).await }
            })
            .await;
            if generation.get() != current {
                return;
            }
            let page = match page {
                Ok(page) => page,
                Err(e) => {
                    if let Some(ui) = weak.upgrade() {
                        ui.report_error("Could not load items", &e);
                    }
                    return;
                }
            };
            let received = page.items.len();
            let objects: Vec<_> = page
                .items
                .into_iter()
                .filter(|item| allowed.contains(&item.id))
                .map(glib::BoxedAnyObject::new)
                .collect();
            shown += objects.len();
            store.extend_from_slice(&objects);
            query.start += received;
            if received == 0 || query.start >= page.total_record_count {
                break;
            }
        }
        if let Some(count) = count.upgrade() {
            count.set_label(&items_label(shown));
        }
    });
}

/// Fills `store` with `query`'s pages while it's still the current one.
fn load_query(
    ui: &Ui,
    store: &gtk::gio::ListStore,
    count: &gtk::Label,
    query: ItemQuery,
    generation: &Rc<Cell<u64>>,
    current: u64,
) {
    let still_current = generation.clone();
    let count = count.downgrade();
    library::load_pages(
        ui,
        store,
        query,
        usize::MAX,
        move || still_current.get() == current,
        move |total| {
            if let Some(count) = count.upgrade() {
                count.set_label(&items_label(total));
            }
        },
    );
}

fn items_label(total: usize) -> String {
    match total {
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    }
}

/// Genre or tag tiles; each opens a sortable grid of that genre/tag.
fn chips(ui: &Ui, library_id: &str, item_type: &'static str, kind: Chips) -> gtk::Widget {
    let flow = super::category_tile::flow(ui);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&loading());
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .vexpand(true)
        .build();

    let client = ui.client();
    let user_id = ui.user_id();
    let library_id = library_id.to_string();
    let weak = ui.downgrade();
    let tiles = flow.clone();
    glib::spawn_future_local(async move {
        let flow = tiles;
        let fetch_library = library_id.clone();
        let result = spawn_tokio(async move {
            match kind {
                Chips::Genres => client.genres(&user_id, &fetch_library, item_type).await,
                Chips::Tags => client.tags(&user_id, &fetch_library, item_type).await,
            }
        })
        .await;
        let Some(ui) = weak.upgrade() else { return };
        clear(&content);
        let entries = match result {
            Ok(entries) => entries,
            Err(e) => {
                ui.report_error("Could not load this tab", &e);
                return;
            }
        };
        if entries.is_empty() {
            let title = match kind {
                Chips::Genres => "No genres in this library",
                Chips::Tags => "No tags in this library",
            };
            content.append(
                &adw::StatusPage::builder()
                    .icon_name(crate::ui::icons::GENRES)
                    .title(title)
                    .vexpand(true)
                    .build(),
            );
            return;
        }
        for entry in entries {
            let mut query = ItemQuery {
                parent_id: Some(library_id.clone()),
                include_types: Some(item_type),
                recursive: true,
                ..Default::default()
            };
            match kind {
                Chips::Genres => query.genre_id = Some(entry.id.clone()),
                Chips::Tags => query.tag_id = Some(entry.id.clone()),
            }
            flow.append(&super::category_tile::tile(&ui, &entry.name, query));
        }
        super::category_tile::fit(&flow, flow.observe_children().n_items() as usize);
        content.append(&flow);
    });
    scrolled.upcast()
}

/// How many recently watched series seed "Because you watched" rows.
const SEED_SERIES: usize = 3;
const SUGGESTION_GENRES: usize = 2;

/// TV has no server-side recommendations, so build them from what you
/// watched: titles similar to your recent series, and series from the
/// genres you watch most. Rows never repeat a title. Watched ones are left
/// out when the account says so (Emby's "hide watched in Suggestions",
/// which the server applies only to its own movie recommendations).
async fn tv_suggestions(
    client: &crate::emby::EmbyClient,
    user_id: &str,
    library_id: &str,
) -> anyhow::Result<Vec<Recommendation>> {
    let hide_played = crate::account::hide_played_in_suggestions();
    // Recently played episodes give the series you've been watching.
    let recent = client
        .items(
            user_id,
            &ItemQuery {
                parent_id: Some(library_id.to_string()),
                include_types: Some("Episode"),
                recursive: true,
                filters: vec!["IsPlayed"],
                sort_by: Some("DatePlayed"),
                descending: true,
                limit: 60,
                ..Default::default()
            },
        )
        .await?
        .items;
    let mut series_ids: Vec<(String, String)> = Vec::new();
    for episode in &recent {
        if let (Some(id), Some(name)) = (&episode.series_id, &episode.series_name)
            && !series_ids.iter().any(|(seen, _)| seen == id)
        {
            series_ids.push((id.clone(), name.clone()));
        }
    }

    let mut seen: std::collections::HashSet<String> =
        series_ids.iter().map(|(id, _)| id.clone()).collect();
    let mut rows = Vec::new();
    for (id, name) in series_ids.iter().take(SEED_SERIES) {
        let similar = client.similar(user_id, id, 16).await.unwrap_or_default();
        let items = fresh(similar, hide_played, &mut seen);
        if !items.is_empty() {
            rows.push(Recommendation {
                recommendation_type: "SimilarToRecentlyPlayed".into(),
                baseline_item_name: Some(name.clone()),
                items,
            });
        }
    }

    // Genres of the series you watch, most common first.
    let mut watched_series = Vec::new();
    for (id, _) in series_ids.iter().take(12) {
        if let Ok(series) = client.item(user_id, id).await {
            watched_series.push(series);
        }
    }
    for (genre_id, genre) in top_genres(&watched_series, SUGGESTION_GENRES) {
        let page = client
            .items(
                user_id,
                &ItemQuery {
                    parent_id: Some(library_id.to_string()),
                    include_types: Some("Series"),
                    recursive: true,
                    genre_id: Some(genre_id),
                    filters: if hide_played {
                        vec!["IsUnplayed"]
                    } else {
                        Vec::new()
                    },
                    sort_by: Some("Random"),
                    limit: 16,
                    ..Default::default()
                },
            )
            .await;
        let items = fresh(
            page.map(|p| p.items).unwrap_or_default(),
            hide_played,
            &mut seen,
        );
        if !items.is_empty() {
            rows.push(Recommendation {
                recommendation_type: "Genre".into(),
                baseline_item_name: Some(genre),
                items,
            });
        }
    }
    Ok(rows)
}

/// Items not shown in an earlier row (marking them shown), watched ones
/// dropped if `hide_played`.
fn fresh(
    items: Vec<BaseItem>,
    hide_played: bool,
    seen: &mut std::collections::HashSet<String>,
) -> Vec<BaseItem> {
    items
        .into_iter()
        .filter(|item| !(hide_played && item.played()) && seen.insert(item.id.clone()))
        .take(12)
        .collect()
}

/// The `limit` most common genres across `items`, as (id, name).
fn top_genres(items: &[BaseItem], limit: usize) -> Vec<(String, String)> {
    let mut counts: Vec<(String, String, usize)> = Vec::new();
    for genre in items.iter().flat_map(|item| &item.genre_items) {
        match counts.iter_mut().find(|(id, _, _)| *id == genre.id) {
            Some(entry) => entry.2 += 1,
            None => counts.push((genre.id.clone(), genre.name.clone(), 1)),
        }
    }
    // Most common first; ties keep first-seen order (recent series first).
    counts.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    counts
        .into_iter()
        .take(limit)
        .map(|(id, name, _)| (id, name))
        .collect()
}

struct Suggestions {
    resume: Vec<BaseItem>,
    next_up: Vec<BaseItem>,
    latest: Vec<BaseItem>,
    recommendations: Vec<Recommendation>,
}

/// The library's own home: what's in progress, what's new, and what's
/// similar to what you watched.
fn suggestions(ui: &Ui, folder: &BaseItem, kind: Kind) -> gtk::Widget {
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
    let library_id = folder.id.clone();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let fetch_library = library_id.clone();
        let result = spawn_tokio(async move {
            let parent = Some(fetch_library.as_str());
            let (resume, next_up, latest, recommendations) = tokio::join!(
                client.resume(&user_id, parent, SUGGESTION_ROW_LIMIT),
                async {
                    match kind {
                        Kind::Shows => client.next_up(&user_id, parent, SUGGESTION_ROW_LIMIT).await,
                        Kind::Movies => Ok(Vec::new()),
                    }
                },
                client.latest(&user_id, &fetch_library, SUGGESTION_ROW_LIMIT),
                async {
                    match kind {
                        Kind::Movies => {
                            client
                                .movie_recommendations(
                                    &user_id,
                                    &fetch_library,
                                    RECOMMENDATION_ROWS,
                                    RECOMMENDATION_ITEMS,
                                )
                                .await
                        }
                        Kind::Shows => tv_suggestions(&client, &user_id, &fetch_library).await,
                    }
                },
            );
            // Rows are independent; one failing shouldn't hide the others.
            let or_empty = |name: &str, result: anyhow::Result<Vec<_>>| {
                result.unwrap_or_else(|e| {
                    tracing::warn!("{name} row failed: {e:#}");
                    Vec::new()
                })
            };
            Suggestions {
                resume: or_empty("resume", resume),
                next_up: or_empty("next up", next_up),
                latest: or_empty("latest", latest),
                recommendations: recommendations.unwrap_or_else(|e| {
                    tracing::warn!("recommendations failed: {e:#}");
                    Vec::new()
                }),
            }
        })
        .await;
        let Some(ui) = weak.upgrade() else { return };
        clear(&content);

        let mut any = false;
        let mut add = |widget: gtk::Box| {
            content.append(&widget);
            any = true;
        };
        if !result.resume.is_empty() {
            add(row(
                &ui,
                "Continue Watching",
                &result.resume,
                Shape::for_episodes(),
                Click::Resume,
                More::Resume(Some(library_id.clone())),
            ));
        }
        if !result.next_up.is_empty() {
            add(row(
                &ui,
                "Next Up",
                &result.next_up,
                Shape::for_episodes(),
                Click::Open,
                More::NextUp(Some(library_id.clone())),
            ));
        }
        if !result.latest.is_empty() {
            add(row(
                &ui,
                "Latest",
                &result.latest,
                Shape::Poster,
                Click::Open,
                More::None,
            ));
        }
        for recommendation in result.recommendations {
            if recommendation.items.is_empty() {
                continue;
            }
            add(row(
                &ui,
                &recommendation.title(),
                &recommendation.items,
                Shape::Poster,
                Click::Open,
                More::None,
            ));
        }
        if !any {
            content.append(
                &adw::StatusPage::builder()
                    .icon_name(crate::ui::icons::SUGGESTIONS)
                    .title("Nothing to suggest yet")
                    .description("Watch something in this library first")
                    .vexpand(true)
                    .build(),
            );
        }
    });
    scrolled.upcast()
}

const UPCOMING_LIMIT: usize = 100;

/// Episodes that haven't aired yet, one row per air day.
fn upcoming(ui: &Ui, library_id: &str) -> gtk::Widget {
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
    let library_id = library_id.to_string();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move {
            client
                .upcoming(&user_id, Some(&library_id), UPCOMING_LIMIT)
                .await
        })
        .await;
        let Some(ui) = weak.upgrade() else { return };
        clear(&content);
        let items = match result {
            Ok(items) => items,
            Err(e) => {
                ui.report_error("Could not load upcoming episodes", &e);
                return;
            }
        };
        if items.is_empty() {
            content.append(
                &adw::StatusPage::builder()
                    .icon_name(crate::ui::icons::UPCOMING)
                    .title("Nothing upcoming")
                    .description("New episodes of your shows appear here before they air")
                    .vexpand(true)
                    .build(),
            );
            return;
        }
        let today = glib::DateTime::now_local().ok();
        let mut days: Vec<(String, Vec<BaseItem>)> = Vec::new();
        for item in items {
            let day = air_date(&item)
                .map(|date| super::history::day_label(&date, today.as_ref()))
                .unwrap_or_else(|| "Date unknown".into());
            match days.last_mut() {
                Some((last, group)) if *last == day => group.push(item),
                _ => days.push((day, vec![item])),
            }
        }
        for (day, group) in days {
            content.append(&row(
                &ui,
                &day,
                &group,
                Shape::for_episodes(),
                Click::Open,
                More::None,
            ));
        }
    });
    scrolled.upcast()
}

/// The day an episode airs. Emby stores air dates as midnight UTC, so the
/// date is taken as is: converting to local time would move it a day back
/// west of UTC.
fn air_date(item: &BaseItem) -> Option<glib::DateTime> {
    let text = item.premiere_date.as_deref()?;
    let utc = glib::DateTime::from_iso8601(text, Some(&glib::TimeZone::utc())).ok()?;
    glib::DateTime::new(
        &glib::TimeZone::local(),
        utc.year(),
        utc.month(),
        utc.day_of_month(),
        0,
        0,
        0.0,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::{Extra, IdRestriction, civil, fresh, intersect, iso, top_genres};
    use crate::emby::browse::ItemQuery;
    use crate::emby::models::{BaseItem, NameId, UserItemData};

    fn series(id: &str, genres: &[&str]) -> BaseItem {
        BaseItem {
            id: id.into(),
            genre_items: genres
                .iter()
                .map(|g| NameId {
                    id: format!("g-{g}"),
                    name: g.to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn top_genres_counts_across_watched_series() {
        let watched = [
            series("a", &["Fantasy", "Comedy"]),
            series("b", &["Comedy"]),
            series("c", &["Action", "Comedy", "Fantasy"]),
        ];
        let top: Vec<String> = top_genres(&watched, 2)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(top, ["Comedy", "Fantasy"]);
        assert!(top_genres(&[], 2).is_empty());
    }

    #[test]
    fn rows_skip_watched_and_repeated_titles() {
        let mut seen = std::collections::HashSet::from(["a".to_string()]);
        let mut watched = series("w", &[]);
        watched.user_data = Some(UserItemData {
            played: true,
            ..Default::default()
        });
        let ids = |row: Vec<BaseItem>| row.into_iter().map(|i| i.id).collect::<Vec<_>>();
        let row = fresh(
            vec![series("a", &[]), watched.clone(), series("x", &[])],
            true,
            &mut seen,
        );
        assert_eq!(ids(row), ["x"]);
        assert!(fresh(vec![series("x", &[])], true, &mut seen).is_empty());
        // Watched titles stay when the account doesn't hide them.
        assert_eq!(ids(fresh(vec![watched], false, &mut seen)), ["w"]);
    }

    #[test]
    fn extra_filters_round_trip_through_their_saved_form() {
        let extra = Extra {
            decade: Some(2010),
            hd: Some(true),
            subtitles: true,
            rating: Some("PG-13".into()),
            added_days: Some(30),
        };
        let saved = extra.encode().unwrap();
        assert_eq!(saved, "v1;decade=2010;res=hd;subs=1;rating=PG-13;added=30");
        assert_eq!(Extra::decode(&saved), extra);
        assert_eq!(Extra::default().encode(), None);
        assert_eq!(extra.active(), 5);
        assert_eq!(Extra::default().active(), 0);
    }

    #[test]
    fn saved_extra_filters_are_read_tolerantly() {
        // Unknown keys and invalid values are left off; the rest still count.
        let extra = Extra::decode("v1;future=9;decade=1850;res=4k;subs=1;added=45;rating=R");
        assert_eq!(
            extra,
            Extra {
                subtitles: true,
                rating: Some("R".into()),
                ..Default::default()
            }
        );
        // Another version, or none, is no filters.
        assert_eq!(Extra::decode("v2;subs=1"), Extra::default());
        assert_eq!(Extra::decode("subs=1"), Extra::default());
        assert_eq!(Extra::decode(""), Extra::default());
        assert_eq!(Extra::decode("v1;;=;rating=").rating, None);
        assert_eq!(Extra::decode("v1;res=sd").hd, Some(false));
    }

    #[test]
    fn decades_list_their_years() {
        assert_eq!(
            Extra::years(2010, 2026),
            "2010,2011,2012,2013,2014,2015,2016,2017,2018,2019"
        );
        // The current decade stops at this year.
        assert_eq!(Extra::years(2020, 2023), "2020,2021,2022,2023");
        let older = Extra::years(0, 2026);
        assert!(older.starts_with("1900,1901,"));
        assert!(older.ends_with(",1968,1969"));
        assert_eq!(older.split(',').count(), 70);
    }

    #[test]
    fn extra_filters_go_into_the_query_by_what_the_server_can_do() {
        let extra = Extra {
            decade: Some(2020),
            hd: Some(false),
            subtitles: true,
            rating: Some("TV-MA".into()),
            added_days: None,
        };
        let mut movies = ItemQuery::default();
        extra.apply(&mut movies, false, 2022);
        assert_eq!(movies.years.as_deref(), Some("2020,2021,2022"));
        assert_eq!(movies.official_ratings.as_deref(), Some("TV-MA"));
        assert_eq!(movies.is_hd, Some(false));
        assert_eq!(movies.has_subtitles, Some(true));
        // A series has no streams of its own: those go by its episodes.
        let mut shows = ItemQuery::default();
        extra.apply(&mut shows, true, 2022);
        assert_eq!(shows.years, movies.years);
        assert_eq!((shows.is_hd, shows.has_subtitles), (None, None));
        let restriction = IdRestriction::new(None, true, false, &extra, 0);
        assert_eq!(restriction.episodes, Some((Some(false), Some(true))));
        let movie_restriction = IdRestriction::new(None, false, false, &extra, 0);
        assert!(movie_restriction.is_none());
    }

    #[test]
    fn restrictions_cover_progress_and_added() {
        assert!(IdRestriction::new(None, true, false, &Extra::default(), 0).is_none());
        assert!(!IdRestriction::new(None, true, true, &Extra::default(), 0).is_none());
        let added = Extra {
            added_days: Some(7),
            ..Default::default()
        };
        let restriction = IdRestriction::new(Some("lib".into()), false, false, &added, 8 * 86_400);
        assert_eq!(
            restriction.added_since.as_deref(),
            Some("1970-01-02T00:00:00")
        );
        assert_eq!(restriction.parent.as_deref(), Some("lib"));
    }

    #[test]
    fn id_sets_intersect_in_the_first_ones_order() {
        let set = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            intersect(vec![set(&["c", "a", "b"]), set(&["b", "c", "z"])]),
            ["c", "b"]
        );
        assert_eq!(
            intersect(vec![set(&["c", "a"]), set(&["a"]), set(&["a", "c"])]),
            ["a"]
        );
        assert_eq!(intersect(vec![set(&["a"])]), ["a"]);
        assert!(intersect(vec![set(&["a"]), set(&[])]).is_empty());
        assert!(intersect(Vec::new()).is_empty());
    }

    #[test]
    fn utc_dates_come_out_right() {
        assert_eq!(iso(0), "1970-01-01T00:00:00");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20");
        // A leap day, and the day after.
        assert_eq!(iso(1_709_164_800), "2024-02-29T00:00:00");
        assert_eq!(iso(1_709_251_200), "2024-03-01T00:00:00");
        assert_eq!(civil(1_700_000_000).0, 2023);
        assert_eq!(civil(-86_400).0, 1969);
    }
}
