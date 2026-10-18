//! Movie/episode details with Resume / Play buttons. Episodes also get
//! previous/next links and a strip of their season's episodes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::card::Shape;
use super::hero::Hero;
use super::rows::{Click, More, row};
use super::{Ui, clear, format_timestamp, reload_on_change, scrolled_page};
use crate::emby::models::BaseItem;
use crate::playback::markers::neighbours;
use crate::runtime::spawn_tokio;

struct DetailsView {
    hero: Hero,
    /// Previous/next links and the season strip (episodes only).
    episodes: gtk::Box,
    /// Cast and similar titles.
    related: gtk::Box,
    /// The previous/next episode links, once loaded.
    neighbours: RefCell<Option<gtk::CenterBox>>,
    /// Where the cursor goes after stepping to another episode.
    arrived_at: Cell<Option<Spot>>,
    /// The title shown (episodes change it in place, see [`step`]).
    item: RefCell<BaseItem>,
    /// Bumped by every load, so only the newest one is shown.
    load_generation: Cell<u64>,
}

/// Where the cursor was when stepping to another episode, so it goes back
/// on the same kind of thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    Previous,
    Next,
    Play,
}

pub fn page(ui: &Ui, item: &BaseItem) -> adw::NavigationPage {
    let hero = Hero::new();
    let episodes = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .build();
    let related = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .build();
    hero.extra.append(&episodes);
    hero.extra.append(&related);
    let header = adw::HeaderBar::new();
    let page = scrolled_page(&item.name, None, &header, &hero.root);
    let view = Rc::new(DetailsView {
        hero,
        episodes,
        related,
        neighbours: RefCell::new(None),
        arrived_at: Cell::new(None),
        item: RefCell::new(item.clone()),
        load_generation: Cell::new(0),
    });
    // X / right-click on the page: the title's own menu (download, watched,
    // favourite...). Cards below keep theirs.
    let shown = Rc::downgrade(&view);
    super::card::attach_menu(ui, &view.hero.root, move || {
        shown.upgrade().map(|view| view.item.borrow().clone())
    });

    // Show what we already have right away, then refresh from the server
    // (list results can lack the overview, and watch state may be stale).
    show(ui, &view, item);
    reload_on_change(ui, &page, {
        let weak = ui.downgrade();
        let view = view.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let id = view.item.borrow().id.clone();
                load(&ui, &view, &id);
            }
        }
    });
    load(ui, &view, &item.id);
    page
}

/// Shows `target` (another episode) in this page instead of opening a new
/// one: the current episode stays up until the new one has loaded, then
/// everything changes at once, with no blank frame or scroll jump.
fn step(ui: &Ui, view: &Rc<DetailsView>, target: &BaseItem, spot: Option<Spot>) {
    // The rows below (cast...) keep at least their current height while
    // stepping: an episode without them would shorten the page and scroll it.
    let related = &view.related;
    related.set_height_request(related.height().max(related.height_request()));
    view.arrived_at.set(spot);
    view.item.replace(target.clone());
    load(ui, view, &target.id);
}

fn load(ui: &Ui, view: &Rc<DetailsView>, item_id: &str) {
    let client = ui.client();
    let user_id = ui.user_id();
    let item_id = item_id.to_string();
    let ui = ui.clone();
    let view = view.clone();
    let generation = view.load_generation.get() + 1;
    view.load_generation.set(generation);
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move {
            let item = client.item(&user_id, &item_id).await?;
            // The whole series, for neighbours and the season strip.
            let episodes = match &item.series_id {
                Some(series_id) if item.item_type == "Episode" => client
                    .series_episodes(series_id, &user_id)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("episode list failed: {e:#}");
                        Vec::new()
                    }),
                _ => Vec::new(),
            };
            Ok::<_, anyhow::Error>((item, episodes))
        })
        .await;
        // Stepped on again meanwhile: that load shows instead.
        if view.load_generation.get() != generation {
            return;
        }
        match result {
            Ok((item, episodes)) => {
                view.item.replace(item.clone());
                if let Some(page) = view
                    .hero
                    .root
                    .ancestor(adw::NavigationPage::static_type())
                    .and_downcast::<adw::NavigationPage>()
                {
                    page.set_title(&item.name);
                }
                // Everything below is rebuilt; the cursor comes back to its
                // spot (or, after LB/RB, to the matching one).
                let mark = view
                    .episodes
                    .ancestor(adw::NavigationPage::static_type())
                    .and_then(|page| super::gamepad::mark_cursor(&page));
                show(&ui, &view, &item);
                show_episodes(&ui, &view, &item, &episodes);
                super::hero::show_related(&ui, &view.related, &item);
                if view.arrived_at.get().is_some() {
                    place_cursor(&view);
                    view.arrived_at.set(None);
                } else if let Some(mark) = mark {
                    mark.restore();
                }
            }
            Err(e) => ui.report_error("Could not load details", &e),
        }
    });
}

fn show(ui: &Ui, view: &DetailsView, item: &BaseItem) {
    let series = match (&item.series_name, &item.series_id) {
        (Some(name), Some(id)) => Some(BaseItem {
            id: id.clone(),
            name: name.clone(),
            item_type: "Series".into(),
            ..Default::default()
        }),
        _ => None,
    };
    view.hero.show(ui, item, series);

    let buttons = &view.hero.buttons;
    clear(buttons);
    let resume_ticks = item.resume_ticks();
    if resume_ticks > 0 {
        buttons.append(&play_button(
            ui,
            item,
            &format!("Resume {}", format_timestamp(resume_ticks)),
            resume_ticks,
            true,
        ));
        buttons.append(&play_button(ui, item, "Play from Beginning", 0, false));
    } else {
        buttons.append(&play_button(ui, item, "Play", 0, true));
    }
    if let Some(versions) = super::version_picker::button(item) {
        buttons.append(&versions);
    }
    if let Some(subtitles) = super::subtitle_picker::button(item) {
        buttons.append(&subtitles);
    }
    if let Some(trailer) = trailer_button(ui, item) {
        buttons.append(&trailer);
    }
    for toggle in super::hero::item_toggles(ui, item) {
        buttons.append(&toggle);
    }
    if let Some(download) = super::downloads::button(ui, item) {
        buttons.append(&download);
    }
}

/// Previous/next links and "More in Season N" for an episode.
fn show_episodes(ui: &Ui, view: &Rc<DetailsView>, item: &BaseItem, episodes: &[BaseItem]) {
    clear(&view.episodes);
    if episodes.is_empty() {
        return;
    }
    let (previous, next) = neighbours(episodes, &item.id);
    let nav = gtk::CenterBox::new();
    view.neighbours.replace(Some(nav.clone()));
    // LB/RB step to the previous/next episode, like the buttons below.
    if let Some(page) = view
        .episodes
        .ancestor(adw::NavigationPage::static_type())
        .and_downcast::<adw::NavigationPage>()
    {
        let (weak, shown) = (ui.downgrade(), Rc::downgrade(view));
        let (previous, next) = (previous.clone(), next.clone());
        let nav = nav.downgrade();
        super::set_tab_label(&page, "Episode");
        super::set_tab_stepper(&page, move |forward| {
            let target = if forward { &next } else { &previous };
            let (Some(ui), Some(view), Some(target)) = (weak.upgrade(), shown.upgrade(), target)
            else {
                return;
            };
            step(&ui, &view, target, Some(spot_of(nav.upgrade().as_ref())));
        });
    }
    if let Some(previous) = previous {
        nav.set_start_widget(Some(&neighbour_button(ui, view, &previous, false)));
    }
    if let Some(next) = next {
        nav.set_end_widget(Some(&neighbour_button(ui, view, &next, true)));
    }
    view.episodes.append(&nav);

    let season: Vec<BaseItem> = episodes
        .iter()
        .filter(|e| e.season_id == item.season_id)
        .cloned()
        .collect();
    if season.len() > 1 {
        let title = match item.parent_index_number {
            Some(number) => format!("More in Season {number}"),
            None => "More Episodes".to_string(),
        };
        let series = item.series_id.clone().map(|id| {
            Box::new(BaseItem {
                id,
                name: item.series_name.clone().unwrap_or_default(),
                item_type: "Series".into(),
                ..Default::default()
            })
        });
        let more = series.map_or(More::None, More::Library);
        let (weak, shown) = (ui.downgrade(), Rc::downgrade(view));
        let open = Click::With(Rc::new(move |target: &BaseItem| {
            if let (Some(ui), Some(view)) = (weak.upgrade(), shown.upgrade()) {
                step(&ui, &view, target, None);
            }
        }));
        let strip = row(ui, &title, &season, Shape::Landscape, open, more);
        view.episodes.append(&strip);
        scroll_to_current(&strip, &season, &item.id);
    }
}

/// Starts the season strip with the current episode centred. The scroll
/// range only becomes real once the strip is laid out (and grows as card
/// images arrive), so the position is set when the range changes, not at
/// a guessed moment.
fn scroll_to_current(strip: &gtk::Box, season: &[BaseItem], item_id: &str) {
    let Some(index) = season.iter().position(|e| e.id == item_id) else {
        return;
    };
    let Some(scroller) = strip.last_child().and_downcast::<gtk::ScrolledWindow>() else {
        return;
    };
    let card = f64::from(super::scaled(Shape::Landscape.size().0));
    let step = card + f64::from(super::rows::CARD_SPACING);
    // Cards start after the row's side margin.
    let card_start = f64::from(super::rows::ROW_MARGIN) + step * index as f64;
    let adjustment = scroller.hadjustment();
    let place = move |adjustment: &gtk::Adjustment| {
        let page = adjustment.page_size();
        if page <= 0.0 || adjustment.upper() < card_start + card {
            return false; // not laid out yet
        }
        let centred = card_start - (page - card) / 2.0;
        adjustment.set_value(centred.clamp(0.0, (adjustment.upper() - page).max(0.0)));
        true
    };
    if place(&adjustment) {
        return;
    }
    let handler = std::rc::Rc::new(std::cell::RefCell::new(None));
    let id = adjustment.connect_changed({
        let handler = handler.clone();
        move |adjustment| {
            if place(adjustment)
                && let Some(id) = handler.borrow_mut().take()
            {
                adjustment.disconnect(id);
            }
        }
    });
    handler.replace(Some(id));
}

/// "‹ S1:E3 · Title" / "S1:E5 · Title ›"; shows that episode in this page
/// (see [`step`]).
/// What the cursor is on now: one of the previous/next links, or else
/// anything (Play stands in for it).
fn spot_of(nav: Option<&gtk::CenterBox>) -> Spot {
    let focus = nav.and_then(|nav| nav.root()).and_then(|root| root.focus());
    let within = |widget: Option<gtk::Widget>| {
        widget.is_some_and(|w| focus.as_ref().is_some_and(|f| f == &w || f.is_ancestor(&w)))
    };
    match nav {
        Some(nav) if within(nav.start_widget()) => Spot::Previous,
        Some(nav) if within(nav.end_widget()) => Spot::Next,
        _ => Spot::Play,
    }
}

/// Puts the cursor where it was on the page LB/RB came from.
fn place_cursor(view: &DetailsView) {
    let Some(spot) = view.arrived_at.get() else {
        return;
    };
    let nav = view.neighbours.borrow().clone();
    let target = match spot {
        Spot::Previous => nav.and_then(|nav| nav.start_widget()),
        Spot::Next => nav.and_then(|nav| nav.end_widget()),
        Spot::Play => None,
    };
    // An end of the season has no link on that side: Play instead.
    let target = target.or_else(|| view.hero.buttons.first_child());
    if let Some(target) = target {
        target.grab_focus();
    }
}

fn neighbour_button(
    ui: &Ui,
    view: &Rc<DetailsView>,
    episode: &BaseItem,
    forward: bool,
) -> gtk::Button {
    let label = gtk::Label::builder()
        .label(episode.episode_label())
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(36)
        .build();
    let icon = gtk::Image::from_icon_name(if forward {
        crate::ui::icons::NEXT
    } else {
        crate::ui::icons::BACK
    });
    let content = gtk::Box::builder().spacing(6).build();
    if forward {
        content.append(&label);
        content.append(&icon);
    } else {
        content.append(&icon);
        content.append(&label);
    }
    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(if forward {
            "Next episode"
        } else {
            "Previous episode"
        })
        .css_classes(["flat"])
        .build();
    let (weak, shown) = (ui.downgrade(), Rc::downgrade(view));
    let target = episode.clone();
    button.connect_clicked(move |_| {
        if let (Some(ui), Some(view)) = (weak.upgrade(), shown.upgrade()) {
            let spot = if forward { Spot::Next } else { Spot::Previous };
            step(&ui, &view, &target, Some(spot));
        }
    });
    button
}

/// "Trailer": the movie's own trailer file if it has one, else its first
/// web trailer.
fn trailer_button(ui: &Ui, item: &BaseItem) -> Option<gtk::Button> {
    let local = item.local_trailer_count.unwrap_or(0) > 0;
    let remote = item.remote_trailers.first().map(|t| t.url.clone());
    if !local && remote.is_none() {
        return None;
    }
    let button = gtk::Button::builder()
        .label("Trailer")
        .css_classes(["pill"])
        .build();
    let weak = ui.downgrade();
    let item = item.clone();
    button.connect_clicked(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let title = format!("{} (Trailer)", item.name);
        if !local {
            if let Some(url) = &remote {
                ui.play_link(&title, url);
            }
            return;
        }
        let client = ui.client();
        let user_id = ui.user_id();
        let id = item.id.clone();
        let remote = remote.clone();
        glib::spawn_future_local(async move {
            let found =
                spawn_tokio(async move { client.local_trailers(&user_id, &id).await }).await;
            match (found, remote) {
                (Ok(trailers), _) if !trailers.is_empty() => ui.play(&trailers[0], 0),
                (_, Some(url)) => ui.play_link(&title, &url),
                (Err(e), None) => ui.report_error("Couldn't find the trailer", &e),
                (Ok(_), None) => ui.toast("No trailer found"),
            }
        });
    });
    Some(button)
}

fn play_button(
    ui: &Ui,
    item: &BaseItem,
    label: &str,
    start_ticks: i64,
    primary: bool,
) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .css_classes(if primary {
            vec!["suggested-action", "pill"]
        } else {
            vec!["pill"]
        })
        .build();
    let weak = ui.downgrade();
    let item = item.clone();
    button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.play(&item, start_ticks);
        }
    });
    button
}
