//! The per-episode sync chips (AL / MAL / Kitsu / Trakt): on an episode row
//! of the series page, left of the row's own trailing icons, and after the
//! action buttons on the episode details page.
//!
//! Alignment: a chip is added to the same container as the icons/buttons it
//! sits beside (the row's suffix box, the hero's button box), so the gap
//! between chips is that container's own spacing, and `valign: Center` puts
//! its centre on the container's axis, which is the axis the icons and the
//! round buttons are centred on. Chips are not focusable, so pad navigation
//! doesn't see them.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::icons;
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;
use crate::tracking::markers::{self, Chip, ChipState};
use crate::tracking::tracker;

/// CSS class of every chip (see the stylesheet in `window.rs`).
const CHIP_CLASS: &str = "sync-chip";
/// While an update is still queued the page asks again, this often...
const RECHECK_SECS: u32 = 3;
/// ...and this many times.
const RECHECKS: u32 = 10;

fn state_class(state: ChipState) -> &'static str {
    match state {
        ChipState::OnList => "sync-on",
        ChipState::NotYet => "sync-not-yet",
        ChipState::Queued => "sync-queued",
        ChipState::Unmatched => "sync-unmatched",
    }
}

fn state_icon(state: ChipState) -> Option<&'static str> {
    match state {
        ChipState::OnList => Some(icons::WATCHED),
        ChipState::NotYet => None,
        ChipState::Queued => Some(icons::HISTORY),
        ChipState::Unmatched => Some(icons::UNMATCHED),
    }
}

/// One chip: the service's text and the state's icon.
fn chip_widget(chip: &Chip) -> gtk::Box {
    let widget = gtk::Box::builder()
        .spacing(4)
        .valign(gtk::Align::Center)
        .focusable(false)
        .can_focus(false)
        .tooltip_text(&chip.tooltip)
        .css_classes([CHIP_CLASS, state_class(chip.state)])
        .build();
    widget.append(&gtk::Label::new(Some(markers::label(chip.service))));
    if let Some(icon) = state_icon(chip.state) {
        widget.append(&gtk::Image::from_icon_name(icon));
    }
    widget
}

/// The chips of one episode row. They are suffixes of the row, in front of
/// its trailing icons (✓ and ›), so every gap is the suffix box's own.
pub struct RowChips {
    row: glib::WeakRef<adw::ActionRow>,
    trailing: Vec<gtk::Widget>,
    shown: RefCell<Vec<gtk::Widget>>,
}

impl RowChips {
    /// Adds `trailing` (the ✓ when watched, then ›) as the row's suffixes.
    pub fn new(row: &adw::ActionRow, trailing: Vec<gtk::Widget>) -> Rc<Self> {
        for widget in &trailing {
            row.add_suffix(widget);
        }
        Rc::new(RowChips {
            row: row.downgrade(),
            trailing,
            shown: RefCell::new(Vec::new()),
        })
    }

    fn alive(&self) -> bool {
        self.row.upgrade().is_some_and(|row| row.parent().is_some())
    }

    /// Replaces the chips: they go first, the trailing icons after them.
    fn set(&self, chips: &[Chip]) {
        let Some(row) = self.row.upgrade() else {
            return;
        };
        for old in self.shown.take() {
            row.remove(&old);
        }
        for widget in &self.trailing {
            row.remove(widget);
        }
        let mut shown = Vec::new();
        for chip in chips {
            let widget = chip_widget(chip);
            row.add_suffix(&widget);
            shown.push(widget.upcast());
        }
        for widget in &self.trailing {
            row.add_suffix(widget);
        }
        self.shown.replace(shown);
    }
}

/// A season's rows, with what the markers need to ask again.
struct Page {
    series_id: String,
    series_name: String,
    rows: Vec<(BaseItem, Rc<RowChips>)>,
}

fn any_queued(chips: &[Vec<Chip>]) -> bool {
    chips
        .iter()
        .flatten()
        .any(|chip| chip.state == ChipState::Queued)
}

/// Fills the chips of a season's rows: one read per distinct entry for the
/// whole page.
pub fn show_on_rows(series_id: String, series_name: String, rows: Vec<(BaseItem, Rc<RowChips>)>) {
    if tracker::get().active().is_empty() || rows.is_empty() {
        return;
    }
    refresh_rows(
        Rc::new(Page {
            series_id,
            series_name,
            rows,
        }),
        0,
    );
}

fn refresh_rows(page: Rc<Page>, tries: u32) {
    glib::spawn_future_local(async move {
        let Page {
            series_id,
            series_name,
            rows,
        } = &*page;
        if !rows.iter().any(|(_, chips)| chips.alive()) {
            return;
        }
        let episodes: Vec<BaseItem> = rows.iter().map(|(episode, _)| episode.clone()).collect();
        let (sid, name) = (series_id.clone(), series_name.clone());
        let result =
            spawn_tokio(
                async move { tracker::get().episode_markers(&sid, &name, &episodes).await },
            )
            .await;
        let found = match result {
            Ok(found) => found,
            Err(e) => {
                tracing::debug!("sync markers: {e:#}");
                return;
            }
        };
        let mut shown = Vec::new();
        for (episode, chips) in rows {
            if let Some(list) = found.get(&episode.id) {
                chips.set(list);
                shown.push(list.clone());
            }
        }
        if any_queued(&shown) && tries < RECHECKS {
            let page = page.clone();
            glib::timeout_add_seconds_local_once(RECHECK_SECS, move || {
                refresh_rows(page, tries + 1);
            });
        }
    });
}

/// Fills the chips after the action buttons of an episode's details page
/// (movies and the like have none).
pub fn show_on_buttons(buttons: &gtk::Box, item: &BaseItem) {
    let Some(series_id) = item.series_id.clone() else {
        return;
    };
    if item.item_type != "Episode" || tracker::get().active().is_empty() {
        return;
    }
    // The box is reused for the next episode: its name says whose it is.
    buttons.set_widget_name(&item.id);
    let name = item.series_name.clone().unwrap_or_default();
    refresh_buttons(buttons.downgrade(), item.clone(), series_id, name, 0);
}

fn refresh_buttons(
    buttons: glib::WeakRef<gtk::Box>,
    episode: BaseItem,
    series_id: String,
    series_name: String,
    tries: u32,
) {
    glib::spawn_future_local(async move {
        let (sid, name, items) = (
            series_id.clone(),
            series_name.clone(),
            vec![episode.clone()],
        );
        let result =
            spawn_tokio(async move { tracker::get().episode_markers(&sid, &name, &items).await })
                .await;
        let Some(buttons) = buttons.upgrade() else {
            return;
        };
        if buttons.widget_name() != episode.id {
            return;
        }
        let chips = match result {
            Ok(mut found) => found.remove(&episode.id).unwrap_or_default(),
            Err(e) => {
                tracing::debug!("sync markers: {e:#}");
                return;
            }
        };
        let mut child = buttons.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if widget.has_css_class(CHIP_CLASS) {
                buttons.remove(&widget);
            }
        }
        for chip in &chips {
            buttons.append(&chip_widget(chip));
        }
        if any_queued(&[chips]) && tries < RECHECKS {
            let weak = buttons.downgrade();
            glib::timeout_add_seconds_local_once(RECHECK_SECS, move || {
                refresh_buttons(weak, episode, series_id, series_name, tries + 1);
            });
        }
    });
}
