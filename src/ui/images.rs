//! Server images into `gtk::Picture`s: fetched and decoded off the main
//! thread, kept in a per-process memory cache.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use gtk::prelude::*;
use gtk::{gdk, glib};
use tokio::sync::Semaphore;

use super::{Ui, image_disk_cache};
use crate::emby::EmbyClient;
use crate::emby::browse::ImageRef;
use crate::runtime::spawn_tokio;

/// Concurrent image downloads; a full library grid binds dozens at once.
const MAX_CONCURRENT_FETCHES: usize = 6;

/// Key under which a picture remembers the image it last asked for, so a
/// slow response for a recycled grid cell doesn't overwrite a newer one.
const REQUESTED_KEY: &str = "embyclientplus-requested-image";

/// Decoded textures are kept up to this many bytes (width x height x 4);
/// a poster is ~0.5 MB but a hero backdrop can be tens of MB.
const CACHE_BUDGET: usize = 256 * 1024 * 1024;

thread_local! {
    static CACHE: RefCell<TextureCache> = RefCell::new(TextureCache::default());
}

type TextureCache = ByteLru<gdk::Texture>;

/// Memory cache that evicts the least recently used entries once their
/// total size passes the budget.
struct ByteLru<V> {
    /// Value, size in bytes, last-use tick.
    entries: HashMap<String, (V, usize, u64)>,
    total: usize,
    tick: u64,
    budget: usize,
}

impl<V> Default for ByteLru<V> {
    fn default() -> Self {
        Self::with_budget(CACHE_BUDGET)
    }
}

impl<V> ByteLru<V> {
    fn with_budget(budget: usize) -> Self {
        ByteLru {
            entries: HashMap::new(),
            total: 0,
            tick: 0,
            budget,
        }
    }
}

impl<V: Clone> ByteLru<V> {
    fn get(&mut self, key: &str) -> Option<V> {
        self.tick += 1;
        let (value, _, used) = self.entries.get_mut(key)?;
        *used = self.tick;
        Some(value.clone())
    }

    fn insert(&mut self, key: String, value: V, bytes: usize) {
        self.tick += 1;
        if let Some((_, old, _)) = self.entries.insert(key, (value, bytes, self.tick)) {
            self.total -= old;
        }
        self.total += bytes;
        // The newest entry stays even when it alone is over budget.
        while self.total > self.budget && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some((_, bytes, _)) = self.entries.remove(&oldest) {
                self.total -= bytes;
            }
        }
    }
}

impl ByteLru<gdk::Texture> {
    fn insert_texture(&mut self, key: String, texture: gdk::Texture) {
        let bytes = texture.width().max(0) as usize * texture.height().max(0) as usize * 4;
        self.insert(key, texture, bytes);
    }
}

/// Trims the on-disk image cache, off the main thread.
pub fn prune_disk_cache() {
    std::thread::spawn(image_disk_cache::prune);
}

pub fn clear_cache() {
    CACHE.with(|cache| *cache.borrow_mut() = TextureCache::default());
}

/// Empties the memory and on-disk image caches (the disk part off the main
/// thread). Pictures already showing stay; they re-fetch next load.
pub fn clear_all_caches() {
    clear_cache();
    std::thread::spawn(image_disk_cache::clear);
}

/// Bytes the on-disk image cache takes.
pub async fn disk_cache_size() -> u64 {
    spawn_tokio(async {
        tokio::task::spawn_blocking(image_disk_cache::size)
            .await
            .unwrap_or(0)
    })
    .await
}

/// Shows `image` (resized server-side to `max_width`) in `picture`, or
/// nothing when the item has no such image.
pub fn load(ui: &Ui, picture: &gtk::Picture, image: Option<ImageRef>, max_width: u32) {
    load_with_client(&ui.client(), picture, image, max_width);
}

/// [`load`] for widgets that outlive any one `Ui` (the player page).
pub fn load_with_client(
    client: &Arc<EmbyClient>,
    picture: &gtk::Picture,
    image: Option<ImageRef>,
    max_width: u32,
) {
    // Bigger on screen needs more pixels to stay sharp.
    let max_width = (f64::from(max_width) * super::ui_scale()).ceil() as u32;
    let path = image.map(|image| EmbyClient::image_path(&image, max_width));
    set_requested(picture, path.clone());
    let Some(path) = path else {
        picture.set_paintable(None::<&gdk::Paintable>);
        return;
    };
    if let Some(texture) = CACHE.with(|cache| cache.borrow_mut().get(&path)) {
        picture.set_paintable(Some(&texture));
        return;
    }
    picture.set_paintable(None::<&gdk::Paintable>);

    let client = client.clone();
    let picture = picture.downgrade();
    glib::spawn_future_local(async move {
        let fetch_path = path.clone();
        let texture = spawn_tokio(async move {
            let bytes = cached_bytes(&client, &fetch_path).await?;
            let texture = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))?;
            Ok::<_, anyhow::Error>(texture)
        })
        .await;
        let texture = match texture {
            Ok(texture) => texture,
            Err(e) => {
                tracing::debug!("image {path} failed: {e:#}");
                return;
            }
        };
        CACHE.with(|cache| {
            cache
                .borrow_mut()
                .insert_texture(path.clone(), texture.clone())
        });
        if let Some(picture) = picture.upgrade()
            && requested(&picture).as_deref() == Some(path.as_str())
        {
            picture.set_paintable(Some(&texture));
        }
    });
}

/// The image at `path`: the disk copy while it's fresh, else fetched (and
/// stored). A failed fetch falls back to a stale copy, so images still show
/// offline. Runs on tokio; touches no GTK.
pub async fn cached_bytes(client: &EmbyClient, path: &str) -> anyhow::Result<Vec<u8>> {
    let cache_key = client.url(path);
    let stale = match image_disk_cache::read(&cache_key) {
        Some(cached) if cached.fresh => return Ok(cached.bytes),
        other => other,
    };
    let fetched = async {
        let _permit = semaphore().acquire().await?;
        client.fetch_bytes(path).await
    }
    .await;
    match (fetched, stale) {
        (Ok(bytes), _) => {
            image_disk_cache::write(&cache_key, &bytes);
            Ok(bytes)
        }
        (Err(_), Some(stale)) => Ok(stale.bytes),
        (Err(e), None) => Err(e),
    }
}

fn semaphore() -> &'static Semaphore {
    static SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();
    SEMAPHORE.get_or_init(|| Semaphore::new(MAX_CONCURRENT_FETCHES))
}

fn set_requested(picture: &gtk::Picture, path: Option<String>) {
    // SAFETY: this key is only ever stored and read as `Option<String>`.
    unsafe { picture.set_data(REQUESTED_KEY, path) };
}

fn requested(picture: &gtk::Picture) -> Option<String> {
    // SAFETY: see `set_requested`; the value is cloned out immediately.
    unsafe {
        picture
            .data::<Option<String>>(REQUESTED_KEY)
            .and_then(|path| path.as_ref().clone())
    }
}

/// The on-disk copy of `image` (fetched first if needed), for handing to
/// other programs: MPRIS cover art.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by MPRIS
pub async fn file(
    client: Arc<EmbyClient>,
    image: ImageRef,
    max_width: u32,
) -> Option<std::path::PathBuf> {
    spawn_tokio(async move {
        let path = EmbyClient::image_path(&image, max_width);
        let file = image_disk_cache::file_for(&client.url(&path))?;
        cached_bytes(&client, &path).await.ok()?;
        file.is_file().then_some(file)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::ByteLru;

    #[test]
    fn evicts_least_recently_used_past_the_byte_budget() {
        let mut cache = ByteLru::<u8>::with_budget(100);
        cache.insert("a".into(), 1, 40);
        cache.insert("b".into(), 2, 40);
        assert_eq!(cache.get("a"), Some(1)); // a is now newer than b
        cache.insert("c".into(), 3, 40);
        assert_eq!(cache.get("b"), None);
        assert_eq!(cache.get("a"), Some(1));
        assert_eq!(cache.get("c"), Some(3));
        assert_eq!(cache.total, 80);
    }

    #[test]
    fn one_big_entry_evicts_many_small_ones_and_replacing_recounts() {
        let mut cache = ByteLru::<u8>::with_budget(100);
        for (i, key) in ["a", "b", "c"].into_iter().enumerate() {
            cache.insert(key.into(), i as u8, 30);
        }
        cache.insert("big".into(), 9, 90);
        assert_eq!(cache.entries.len(), 1);
        cache.insert("big".into(), 9, 200); // alone, over budget: kept
        assert_eq!((cache.entries.len(), cache.total), (1, 200));
    }
}
