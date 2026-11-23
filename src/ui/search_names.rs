//! Client-side title matching for search: Emby's `SearchTerm` only matches
//! the start of a word, so a per-session name list finds mid-word hits.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::Ui;
use crate::runtime::spawn_tokio;

/// (id, lowercased name) of every item of one type.
type Names = Rc<Vec<(String, String)>>;

/// Server (if known), user and item type: switching account or server never
/// reuses another's list.
type Key = (Option<String>, String, &'static str);

/// How long a list is reused before the next search refetches it.
const MAX_AGE: Duration = Duration::from_secs(10 * 60);

enum Entry {
    /// A fetch is running; later callers wait for it instead of repeating it.
    Loading(Rc<Notify>),
    Ready(Names, Instant),
}

thread_local! {
    static CACHE: RefCell<HashMap<Key, Entry>> = RefCell::new(HashMap::new());
}

fn is_fresh(fetched: Instant, now: Instant) -> bool {
    now.saturating_duration_since(fetched) < MAX_AGE
}

/// Clears a still-`Loading` entry and wakes the waiters if the fetch fails
/// or its future is dropped, so a failure is never cached.
struct LoadGuard {
    key: Key,
    notify: Rc<Notify>,
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        CACHE.with(|c| {
            let mut cache = c.borrow_mut();
            if matches!(cache.get(&self.key), Some(Entry::Loading(_))) {
                cache.remove(&self.key);
            }
        });
        self.notify.notify_waiters();
    }
}

/// The name list for `key`: cached, joined to a running fetch, or fetched.
async fn names_for(ui: &Ui, key: Key) -> Option<Names> {
    enum Step {
        Use(Names),
        Wait(Rc<Notify>),
        Fetch,
    }
    let step = CACHE.with(|c| match c.borrow().get(&key) {
        Some(Entry::Ready(names, at)) if is_fresh(*at, Instant::now()) => Step::Use(names.clone()),
        Some(Entry::Loading(n)) => Step::Wait(n.clone()),
        _ => Step::Fetch,
    });
    match step {
        Step::Use(names) => Some(names),
        Step::Wait(notify) => {
            // Single-threaded: nothing can notify between the check and here.
            notify.notified().await;
            CACHE.with(|c| match c.borrow().get(&key) {
                Some(Entry::Ready(names, _)) => Some(names.clone()),
                _ => None,
            })
        }
        Step::Fetch => {
            let notify = Rc::new(Notify::new());
            CACHE.with(|c| {
                c.borrow_mut()
                    .insert(key.clone(), Entry::Loading(notify.clone()))
            });
            let guard = LoadGuard {
                key: key.clone(),
                notify,
            };
            let (client, user_id, item_type) = (ui.client(), ui.user_id(), key.2);
            let fetched =
                spawn_tokio(async move { client.item_names(&user_id, item_type).await }).await;
            match fetched {
                Ok(list) => {
                    let names: Names = Rc::new(
                        list.into_iter()
                            .map(|n| (n.id, n.name.to_lowercase()))
                            .collect(),
                    );
                    CACHE.with(|c| {
                        c.borrow_mut()
                            .insert(key, Entry::Ready(names.clone(), Instant::now()))
                    });
                    drop(guard);
                    Some(names)
                }
                Err(e) => {
                    tracing::warn!("search name list failed: {e:#}");
                    None
                }
            }
        }
    }
}

/// Kinds that get mid-word matching.
const TYPES: [&str; 3] = ["Movie", "Series", "Episode"];

/// The searchable types among a comma-separated list.
pub fn matching_types(types: &str) -> Vec<&'static str> {
    TYPES
        .into_iter()
        .filter(|t| types.split(',').any(|x| x == *t))
        .collect()
}

/// Ids of items of `item_type` whose name contains `term` anywhere, not in
/// `skip`, at most `limit`. Empty on a fetch error (Emby's results still show).
pub async fn find(
    ui: &Ui,
    item_type: &'static str,
    term: &str,
    skip: &HashSet<String>,
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let key = (ui.server_id().map(str::to_string), ui.user_id(), item_type);
    let Some(names) = names_for(ui, key).await else {
        return Vec::new();
    };
    contains_matches(&names, term, skip, limit)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Ids, in list order, of names (already lowercase) containing `term`.
fn contains_matches<'a>(
    names: &'a [(String, String)],
    term: &str,
    skip: &HashSet<String>,
    limit: usize,
) -> Vec<&'a str> {
    let term = term.trim().to_lowercase();
    if term.chars().count() < 2 {
        return Vec::new();
    }
    names
        .iter()
        .filter(|(id, name)| name.contains(&term) && !skip.contains(id))
        .map(|(id, _)| id.as_str())
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<(String, String)> {
        [
            ("1", "aruvxq"),
            ("2", "zqharuvx"),
            ("3", "qzvaru"),
            ("4", "other"),
        ]
        .into_iter()
        .map(|(i, n)| (i.to_string(), n.to_string()))
        .collect()
    }

    #[test]
    fn lists_expire_after_ten_minutes() {
        let t = Instant::now();
        assert!(is_fresh(t, t + Duration::from_secs(599)));
        assert!(!is_fresh(t, t + MAX_AGE));
    }

    #[test]
    fn finds_mid_word_in_list_order() {
        assert_eq!(
            contains_matches(&names(), "aru", &HashSet::new(), 10),
            ["1", "2", "3"]
        );
    }

    #[test]
    fn term_is_case_insensitive_and_trimmed() {
        assert_eq!(
            contains_matches(&names(), "  ARU ", &HashSet::new(), 10).len(),
            3
        );
    }

    #[test]
    fn skips_and_limits() {
        let skip = HashSet::from(["1".to_string()]);
        assert_eq!(contains_matches(&names(), "aru", &skip, 10), ["2", "3"]);
        assert_eq!(contains_matches(&names(), "aru", &HashSet::new(), 1), ["1"]);
    }

    #[test]
    fn short_term_and_empty_list() {
        assert!(contains_matches(&names(), "a", &HashSet::new(), 10).is_empty());
        assert!(contains_matches(&names(), " a ", &HashSet::new(), 10).is_empty());
        assert!(contains_matches(&[], "aru", &HashSet::new(), 10).is_empty());
    }
}
