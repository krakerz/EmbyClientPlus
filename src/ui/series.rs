//! Series overview: hero with a "play next" button, season picker,
//! episode list.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::hero::Hero;
use super::{
    Ui, clear, fixed_picture, format_runtime, format_timestamp, images, loading, reload_on_change,
    scrolled_page,
};
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

struct SeriesView {
    hero: Hero,
    seasons: Rc<SeasonChips>,
    episodes: gtk::ListBox,
    /// Cast and similar titles.
    related: gtk::Box,
    /// Seasons backing the chips, in their order.
    season_items: RefCell<Vec<BaseItem>>,
    /// The episode last opened (season id, row index), so coming back
    /// puts the cursor on it again.
    opened: RefCell<Option<(String, usize)>>,
}

type SeasonChanged = Box<dyn Fn(usize)>;

/// One pill per season in a horizontally scrolling row; exactly one is
/// active. Replaces a dropdown so all seasons are visible at a glance.
struct SeasonChips {
    root: gtk::ScrolledWindow,
    row: gtk::Box,
    buttons: RefCell<Vec<gtk::ToggleButton>>,
    selected: Cell<usize>,
    /// Set while chips are rebuilt or moved programmatically.
    quiet: Cell<bool>,
    on_change: RefCell<Option<SeasonChanged>>,
}

impl SeasonChips {
    fn new() -> Rc<Self> {
        let row = gtk::Box::builder().spacing(8).build();
        let root = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&row)
            .build();
        Rc::new(SeasonChips {
            root,
            row,
            buttons: RefCell::new(Vec::new()),
            selected: Cell::new(0),
            quiet: Cell::new(false),
            on_change: RefCell::new(None),
        })
    }

    fn selected(&self) -> usize {
        self.selected.get()
    }

    /// Rebuilds the chips with `selected` active, without notifying.
    fn set_items(self: &Rc<Self>, names: &[&str], selected: usize) {
        clear(&self.row);
        self.quiet.set(true);
        let mut buttons = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let button = gtk::ToggleButton::builder()
                .label(*name)
                .css_classes(["pill", "season-chip"])
                .active(index == selected)
                .build();
            if let Some(first) = buttons.first() {
                button.set_group(Some(first));
            }
            let weak = Rc::downgrade(self);
            button.connect_toggled(move |button| {
                let Some(chips) = weak.upgrade() else { return };
                if !button.is_active() || chips.quiet.get() {
                    return;
                }
                chips.selected.set(index);
                if let Some(on_change) = chips.on_change.borrow().as_ref() {
                    on_change(index);
                }
            });
            self.row.append(&button);
            buttons.push(button);
        }
        self.selected.set(selected);
        self.buttons.replace(buttons);
        self.quiet.set(false);
    }

    /// Moves to the next/previous season (controller bumpers), notifying.
    fn step(&self, forward: bool) {
        let buttons = self.buttons.borrow();
        let next = if forward {
            (self.selected.get() + 1).min(buttons.len().saturating_sub(1))
        } else {
            self.selected.get().saturating_sub(1)
        };
        if let Some(button) = buttons.get(next) {
            button.set_active(true);
        }
    }
}

pub fn page(ui: &Ui, series: &BaseItem) -> adw::NavigationPage {
    let hero = Hero::new();
    let seasons = SeasonChips::new();
    let related = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .build();
    let episodes = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    episodes.append(&loading());
    hero.extra.append(&seasons.root);
    hero.extra.append(&episodes);
    hero.extra.append(&related);
    hero.show(ui, series, None);

    let header = adw::HeaderBar::new();
    let page = scrolled_page(&series.name, None, &header, &hero.root);
    let view = Rc::new(SeriesView {
        hero,
        seasons,
        episodes,
        related,
        season_items: RefCell::new(Vec::new()),
        opened: RefCell::new(None),
    });

    let weak = ui.downgrade();
    let series_id = series.id.clone();
    let on_change: SeasonChanged = Box::new({
        // Weak: the chips live inside the view.
        let view = Rc::downgrade(&view);
        let series_id = series_id.clone();
        move |index| {
            let (Some(ui), Some(view)) = (weak.upgrade(), view.upgrade()) else {
                return;
            };
            let season = view.season_items.borrow().get(index).cloned();
            if let Some(season) = season {
                load_episodes(&ui, &view, &series_id, &season.id);
            }
        }
    });
    view.seasons.on_change.replace(Some(on_change));
    let chips = Rc::downgrade(&view.seasons);
    super::set_tab_stepper(&page, move |forward| {
        if let Some(chips) = chips.upgrade() {
            chips.step(forward);
        }
    });

    reload_on_change(ui, &page, {
        let weak = ui.downgrade();
        let view = view.clone();
        let series_id = series_id.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &view, &series_id);
            }
        }
    });
    load(ui, &view, &series_id);
    // Episodes may load before the page has finished sliding in, and the
    // navigation view moves focus when it does; place the cursor again.
    page.connect_shown({
        let view = Rc::downgrade(&view);
        move |_| {
            if let Some(view) = view.upgrade() {
                let season = view
                    .season_items
                    .borrow()
                    .get(view.seasons.selected())
                    .map(|season| season.id.clone());
                if let Some(season) = season {
                    focus_episode(&view, &season);
                }
            }
        }
    });
    page
}

fn load(ui: &Ui, view: &Rc<SeriesView>, series_id: &str) {
    let client = ui.client();
    let user_id = ui.user_id();
    let series_id = series_id.to_string();
    let ui = ui.clone();
    let view = view.clone();
    glib::spawn_future_local(async move {
        let result = spawn_tokio({
            let series_id = series_id.clone();
            async move {
                let (series, seasons, all) = tokio::join!(
                    client.item(&user_id, &series_id),
                    client.seasons(&series_id, &user_id),
                    client.series_episodes(&series_id, &user_id),
                );
                // The play-next button is optional; the page works without it.
                let all = all.unwrap_or_else(|e| {
                    tracing::warn!("episode list failed: {e:#}");
                    Vec::new()
                });
                Ok::<_, anyhow::Error>((series?, seasons?, all))
            }
        })
        .await;
        let (series, seasons, all) = match result {
            Ok(data) => data,
            Err(e) => {
                clear_list(&view.episodes);
                ui.report_error("Could not load series", &e);
                return;
            }
        };

        view.hero.show(&ui, &series, None);
        clear(&view.hero.buttons);
        if let Some(episode) = next_to_watch(&all) {
            view.hero.buttons.append(&play_next_button(&ui, episode));
        }
        for toggle in super::hero::item_toggles(&ui, &series) {
            view.hero.buttons.append(&toggle);
        }
        super::hero::show_related(&ui, &view.related, &series);

        // Keep the user's current season across reloads; otherwise start at
        // the first one with something left to watch.
        let previous = view
            .season_items
            .borrow()
            .get(view.seasons.selected())
            .map(|season| season.id.clone());
        let selected = previous
            .and_then(|id| seasons.iter().position(|season| season.id == id))
            .unwrap_or_else(|| default_season(&seasons));
        let names: Vec<&str> = seasons.iter().map(|season| season.name.as_str()).collect();
        view.season_items.replace(seasons.clone());
        view.seasons.set_items(&names, selected);
        view.seasons.root.set_visible(seasons.len() > 1);
        match seasons.get(selected) {
            Some(season) => load_episodes(&ui, &view, &series_id, &season.id),
            None => clear_list(&view.episodes),
        }
    });
}

fn default_season(seasons: &[BaseItem]) -> usize {
    seasons
        .iter()
        .position(|season| {
            season
                .user_data
                .as_ref()
                .and_then(|data| data.unplayed_item_count)
                .is_some_and(|count| count > 0)
        })
        .unwrap_or(0)
}

fn load_episodes(ui: &Ui, view: &Rc<SeriesView>, series_id: &str, season_id: &str) {
    let client = ui.client();
    let user_id = ui.user_id();
    let series_id = series_id.to_string();
    let season_id = season_id.to_string();
    let ui = ui.clone();
    let view = view.clone();
    glib::spawn_future_local(async move {
        let wanted = season_id.clone();
        let result =
            spawn_tokio(async move { client.episodes(&series_id, &season_id, &user_id).await })
                .await;
        // The user may have switched seasons while this was loading.
        let current = view
            .season_items
            .borrow()
            .get(view.seasons.selected())
            .map(|season| season.id.clone());
        if current.as_deref() != Some(wanted.as_str()) {
            return;
        }
        clear_list(&view.episodes);
        match result {
            Ok(episodes) => {
                for (index, episode) in episodes.iter().enumerate() {
                    let row = episode_row(&ui, episode);
                    let (opened, season) = (Rc::downgrade(&view), wanted.clone());
                    row.connect_activated(move |_| {
                        if let Some(view) = opened.upgrade() {
                            view.opened.replace(Some((season.clone(), index)));
                        }
                    });
                    view.episodes.append(&row);
                }
                focus_episode(&view, &wanted);
            }
            Err(e) => ui.report_error("Could not load episodes", &e),
        }
    });
}

fn episode_row(ui: &Ui, episode: &BaseItem) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&episode.episode_label()))
        .activatable(true)
        .build();
    let mut details = Vec::new();
    if let Some(ticks) = episode.run_time_ticks {
        details.push(format_runtime(ticks));
    }
    if let (Some(fraction), Some(ticks)) = (episode.progress(), episode.run_time_ticks) {
        let remaining = (ticks as f64 * (1.0 - fraction)) as i64;
        details.push(format!("{} left", format_runtime(remaining)));
    }
    row.set_subtitle(&glib::markup_escape_text(&details.join(" · ")));

    let thumb = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .build();
    let frame = gtk::Overlay::builder()
        .child(&fixed_picture(&thumb, 160, 90))
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Center)
        .margin_top(6)
        .margin_bottom(6)
        .css_classes(["card"])
        .build();
    if let Some(fraction) = episode.progress() {
        frame.add_overlay(
            &gtk::ProgressBar::builder()
                .fraction(fraction)
                .valign(gtk::Align::End)
                .margin_start(6)
                .margin_end(6)
                .margin_bottom(10)
                .build(),
        );
    }
    images::load(ui, &thumb, episode.landscape(), 320);
    row.add_prefix(&frame);
    if episode.played() {
        row.add_suffix(&gtk::Image::from_icon_name(crate::ui::icons::WATCHED));
    }
    row.add_suffix(&gtk::Image::from_icon_name(crate::ui::icons::NEXT));

    let weak = ui.downgrade();
    let target = episode.clone();
    row.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.open(&target);
        }
    });
    row
}

/// Puts the cursor on an episode, so the controller starts there instead
/// of somewhere unseen: the one last opened in this season, else the
/// first. Leaves the focus alone while the user is elsewhere on the page
/// (hero buttons, cast); season chips count as "on the episodes".
fn focus_episode(view: &SeriesView, season_id: &str) {
    let Some(page) = view.episodes.ancestor(adw::NavigationPage::static_type()) else {
        return;
    };
    let focus = view.episodes.root().and_then(|root| root.focus());
    let elsewhere_on_page = focus.as_ref().is_some_and(|focus| {
        focus.is_ancestor(&page)
            && !focus.is_ancestor(&view.episodes)
            && !focus.is_ancestor(&view.seasons.root)
    });
    if elsewhere_on_page {
        return;
    }
    let index = match &*view.opened.borrow() {
        Some((season, index)) if season == season_id => *index as i32,
        _ => 0,
    };
    let row = view
        .episodes
        .row_at_index(index)
        .or_else(|| view.episodes.row_at_index(0));
    if let Some(row) = row {
        row.grab_focus();
    }
}

fn clear_list(list: &gtk::ListBox) {
    list.remove_all();
}

/// The episode to offer: the one in progress, else the one after the last
/// watched, else the first unwatched. `None` when everything is watched.
fn next_to_watch(episodes: &[BaseItem]) -> Option<&BaseItem> {
    if let Some(in_progress) = episodes.iter().find(|e| e.resume_ticks() > 0) {
        return Some(in_progress);
    }
    let after_last_watched = episodes
        .iter()
        .rposition(BaseItem::played)
        .map_or(0, |index| index + 1);
    episodes[after_last_watched..]
        .iter()
        .chain(episodes[..after_last_watched].iter())
        .find(|e| !e.played())
}

/// "Resume S1:E3 · 12:34" or "Play S1:E3".
fn play_next_button(ui: &Ui, episode: &BaseItem) -> gtk::Button {
    let resume = episode.resume_ticks();
    let code = match (episode.parent_index_number, episode.index_number) {
        (Some(season), Some(number)) => format!("S{season}:E{number}"),
        _ => episode.name.clone(),
    };
    let label = if resume > 0 {
        format!("Resume {code} · {}", format_timestamp(resume))
    } else {
        format!("Play {code}")
    };
    let button = gtk::Button::builder()
        .label(label)
        .tooltip_text(episode.episode_label())
        .css_classes(["suggested-action", "pill"])
        .build();
    let weak = ui.downgrade();
    let episode = episode.clone();
    button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.play(&episode, episode.resume_ticks());
        }
    });
    button
}

#[cfg(test)]
mod tests {
    use super::next_to_watch;
    use crate::emby::models::{BaseItem, UserItemData};

    fn episode(id: &str, played: bool, position: i64) -> BaseItem {
        BaseItem {
            id: id.into(),
            item_type: "Episode".into(),
            user_data: Some(UserItemData {
                played,
                playback_position_ticks: position,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn in_progress_episode_comes_first() {
        let list = [
            episode("1", true, 0),
            episode("2", false, 0),
            episode("3", false, 50),
        ];
        assert_eq!(next_to_watch(&list).unwrap().id, "3");
    }

    #[test]
    fn continues_after_the_last_watched() {
        let list = [
            episode("1", true, 0),
            episode("2", false, 0),
            episode("3", true, 0),
            episode("4", false, 0),
        ];
        assert_eq!(next_to_watch(&list).unwrap().id, "4");
        let fresh = [episode("1", false, 0), episode("2", false, 0)];
        assert_eq!(next_to_watch(&fresh).unwrap().id, "1");
    }

    #[test]
    fn wraps_to_earlier_gaps_then_gives_up() {
        let list = [episode("1", false, 0), episode("2", true, 0)];
        assert_eq!(next_to_watch(&list).unwrap().id, "1");
        let done = [episode("1", true, 0)];
        assert!(next_to_watch(&done).is_none());
    }
}
