//! Server images into `gtk::Picture`s: fetched and decoded off the main
//! thread, kept in a per-process memory cache.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
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

/// Decoded textures kept in memory; a poster is ~0.5 MB uncompressed, so
/// this bounds the cache to roughly 150 MB.
const CACHE_CAPACITY: usize = 300;

thread_local! {
    static CACHE: RefCell<TextureCache> = RefCell::new(TextureCache::default());
}

/// Evicts the oldest-inserted entry once full.
#[derive(Default)]
struct TextureCache {
    textures: HashMap<String, gdk::Texture>,
    order: VecDeque<String>,
}

impl TextureCache {
    fn get(&self, path: &str) -> Option<gdk::Texture> {
        self.textures.get(path).cloned()
    }

    fn insert(&mut self, path: String, texture: gdk::Texture) {
        if self.textures.insert(path.clone(), texture).is_none() {
            self.order.push_back(path);
        }
        while self.order.len() > CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.textures.remove(&oldest);
            }
        }
    }
}

/// Trims the on-disk image cache, off the main thread.
pub fn prune_disk_cache() {
    std::thread::spawn(image_disk_cache::prune);
}

pub fn clear_cache() {
    CACHE.with(|cache| *cache.borrow_mut() = TextureCache::default());
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
    let path = image.map(|image| EmbyClient::image_path(&image, max_width));
    set_requested(picture, path.clone());
    let Some(path) = path else {
        picture.set_paintable(None::<&gdk::Paintable>);
        return;
    };
    if let Some(texture) = CACHE.with(|cache| cache.borrow().get(&path)) {
        picture.set_paintable(Some(&texture));
        return;
    }
    picture.set_paintable(None::<&gdk::Paintable>);

    let client = client.clone();
    let picture = picture.downgrade();
    glib::spawn_future_local(async move {
        let fetch_path = path.clone();
        let texture = spawn_tokio(async move {
            let cache_key = client.url(&fetch_path);
            let bytes = match image_disk_cache::read(&cache_key) {
                Some(bytes) => bytes,
                None => {
                    let _permit = semaphore().acquire().await?;
                    let bytes = client.fetch_bytes(&fetch_path).await?;
                    image_disk_cache::write(&cache_key, &bytes);
                    bytes
                }
            };
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
        CACHE.with(|cache| cache.borrow_mut().insert(path.clone(), texture.clone()));
        if let Some(picture) = picture.upgrade()
            && requested(&picture).as_deref() == Some(path.as_str())
        {
            picture.set_paintable(Some(&texture));
        }
    });
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
