//! The play queue: the tracks, the order they play in (shuffled or not),
//! and where we are. Pure data; the engine mirrors it into mpv.

use crate::emby::models::BaseItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Queue {
    items: Vec<BaseItem>,
    /// Play order: indexes into `items`.
    order: Vec<usize>,
    /// Position in `order` of the current track.
    pos: usize,
    shuffled: bool,
}

impl Queue {
    pub fn new(items: Vec<BaseItem>, start: usize) -> Self {
        let order = (0..items.len()).collect();
        Queue {
            pos: start.min(items.len().saturating_sub(1)),
            items,
            order,
            shuffled: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn shuffled(&self) -> bool {
        self.shuffled
    }

    pub fn current(&self) -> Option<&BaseItem> {
        self.order.get(self.pos).map(|&i| &self.items[i])
    }

    /// The tracks in play order.
    pub fn entries(&self) -> impl Iterator<Item = &BaseItem> {
        self.order.iter().map(|&i| &self.items[i])
    }

    /// The position after the current one, wrapping with repeat-all.
    pub fn upcoming(&self, repeat: Repeat) -> Option<usize> {
        if self.order.is_empty() {
            return None;
        }
        match repeat {
            Repeat::One => Some(self.pos),
            _ if self.pos + 1 < self.order.len() => Some(self.pos + 1),
            Repeat::All => Some(0),
            Repeat::Off => None,
        }
    }

    pub fn item_at(&self, position: usize) -> Option<&BaseItem> {
        self.order.get(position).map(|&i| &self.items[i])
    }

    /// Moves to the next track; false at the end (repeat off).
    pub fn advance(&mut self, repeat: Repeat) -> bool {
        // Repeat-one is mpv's loop; skipping still moves on.
        let repeat = if repeat == Repeat::One {
            Repeat::All
        } else {
            repeat
        };
        match self.upcoming(repeat) {
            Some(next) => {
                self.pos = next;
                true
            }
            None => false,
        }
    }

    pub fn back(&mut self) -> bool {
        if self.pos == 0 {
            return false;
        }
        self.pos -= 1;
        true
    }

    pub fn jump(&mut self, position: usize) -> bool {
        if position >= self.order.len() {
            return false;
        }
        self.pos = position;
        true
    }

    /// Queues `item` right after the current track.
    pub fn play_next(&mut self, item: BaseItem) {
        self.items.push(item);
        let index = self.items.len() - 1;
        let at = if self.order.is_empty() {
            0
        } else {
            self.pos + 1
        };
        self.order.insert(at, index);
    }

    /// Queues `item` at the end.
    pub fn add(&mut self, item: BaseItem) {
        self.items.push(item);
        self.order.push(self.items.len() - 1);
    }

    /// Removes the track at `position` (not the current one).
    pub fn remove(&mut self, position: usize) -> bool {
        if position >= self.order.len() || position == self.pos {
            return false;
        }
        self.order.remove(position);
        if position < self.pos {
            self.pos -= 1;
        }
        true
    }

    /// Swaps the track at `position` with its neighbour (`up` = earlier).
    pub fn shift(&mut self, position: usize, up: bool) -> bool {
        let other = if up {
            match position.checked_sub(1) {
                Some(other) => other,
                None => return false,
            }
        } else {
            position + 1
        };
        if position >= self.order.len() || other >= self.order.len() {
            return false;
        }
        self.order.swap(position, other);
        if self.pos == position {
            self.pos = other;
        } else if self.pos == other {
            self.pos = position;
        }
        true
    }

    /// Moves the track at `from` to position `to` (drag and drop). The
    /// playing track keeps playing wherever the move leaves it.
    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        if from >= self.order.len() || to >= self.order.len() || from == to {
            return false;
        }
        let playing = self.order[self.pos];
        let item = self.order.remove(from);
        self.order.insert(to, item);
        self.pos = self
            .order
            .iter()
            .position(|&i| i == playing)
            .unwrap_or(self.pos);
        true
    }

    /// Clears everything after the current track.
    pub fn clear_upcoming(&mut self) {
        self.order.truncate(self.pos + 1);
    }

    /// Shuffle keeps the current track playing and randomizes the rest;
    /// unshuffle goes back to the original order at the current track.
    pub fn set_shuffle(&mut self, on: bool, seed: u64) {
        if on == self.shuffled || self.order.is_empty() {
            self.shuffled = on;
            return;
        }
        let current = self.order[self.pos];
        if on {
            let mut rest: Vec<usize> = self
                .order
                .iter()
                .copied()
                .filter(|&i| i != current)
                .collect();
            shuffle(&mut rest, seed);
            self.order = std::iter::once(current).chain(rest).collect();
            self.pos = 0;
        } else {
            let mut natural = self.order.clone();
            natural.sort_unstable();
            self.pos = natural.iter().position(|&i| i == current).unwrap_or(0);
            self.order = natural;
        }
        self.shuffled = on;
    }
}

/// Fisher–Yates with a small xorshift generator (no crate needed).
fn shuffle(values: &mut [usize], seed: u64) {
    let mut state = seed | 1;
    for i in (1..values.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        values.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(names: &[&str]) -> Vec<BaseItem> {
        names
            .iter()
            .map(|n| BaseItem {
                id: n.to_string(),
                name: n.to_string(),
                item_type: "Audio".into(),
                ..Default::default()
            })
            .collect()
    }

    fn ids(queue: &Queue) -> Vec<String> {
        queue.entries().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn advances_and_wraps_only_with_repeat_all() {
        let mut queue = Queue::new(tracks(&["a", "b"]), 0);
        assert!(queue.advance(Repeat::Off));
        assert_eq!(queue.current().unwrap().id, "b");
        assert_eq!(queue.upcoming(Repeat::Off), None);
        assert!(!queue.advance(Repeat::Off));
        assert_eq!(queue.upcoming(Repeat::All), Some(0));
        assert_eq!(queue.upcoming(Repeat::One), Some(1));
        assert!(queue.advance(Repeat::All));
        assert_eq!(queue.current().unwrap().id, "a");
    }

    #[test]
    fn play_next_add_remove_and_shift() {
        let mut queue = Queue::new(tracks(&["a", "b", "c"]), 1);
        queue.play_next(tracks(&["x"]).remove(0));
        queue.add(tracks(&["z"]).remove(0));
        assert_eq!(ids(&queue), ["a", "b", "x", "c", "z"]);
        assert!(!queue.remove(1), "can't remove the playing track");
        assert!(queue.remove(0));
        assert_eq!(queue.current().unwrap().id, "b");
        assert!(queue.shift(3, true));
        assert_eq!(ids(&queue), ["b", "x", "z", "c"]);
        queue.clear_upcoming();
        assert_eq!(ids(&queue), ["b"]);
    }

    #[test]
    fn shuffle_keeps_the_current_track_and_unshuffle_restores_order() {
        let mut queue = Queue::new(tracks(&["a", "b", "c", "d", "e", "f"]), 2);
        queue.set_shuffle(true, 42);
        assert_eq!(queue.current().unwrap().id, "c");
        assert_eq!(queue.position(), 0);
        let mut sorted = ids(&queue);
        sorted.sort();
        assert_eq!(sorted, ["a", "b", "c", "d", "e", "f"]);
        queue.advance(Repeat::Off);
        let playing = queue.current().unwrap().id.clone();
        queue.set_shuffle(false, 0);
        assert_eq!(ids(&queue), ["a", "b", "c", "d", "e", "f"]);
        assert_eq!(queue.current().unwrap().id, playing);
    }

    #[test]
    fn moves_past_the_playing_track_keep_it_playing() {
        let mut queue = Queue::new(tracks(&["0", "1", "2", "3", "4"]), 2);
        // An upcoming track moved above the playing one, step by step.
        assert!(queue.shift(3, true));
        assert_eq!(ids(&queue), ["0", "1", "3", "2", "4"]);
        assert_eq!(queue.current().unwrap().id, "2");
        // A played track dragged below it.
        assert!(queue.move_item(0, 4));
        assert_eq!(ids(&queue), ["1", "3", "2", "4", "0"]);
        assert_eq!(queue.current().unwrap().id, "2");
        assert_eq!(queue.position(), 2);
        // The playing track itself can move too.
        assert!(queue.move_item(2, 0));
        assert_eq!(queue.position(), 0);
        assert_eq!(queue.current().unwrap().id, "2");
        assert!(!queue.move_item(1, 1));
        assert!(!queue.move_item(9, 0));
    }
}
