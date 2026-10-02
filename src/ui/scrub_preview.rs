//! The card above the seek bar while scrubbing: the frame at that point
//! (from Emby's trickplay thumbnails, else the chapter's image), the
//! chapter name and the time.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};

use super::format_timestamp;
use crate::emby::EmbyClient;
use crate::emby::models::{BaseItem, ChapterInfo};
use crate::emby::trickplay::{THUMB_WIDTH, Thumbnails};
use crate::playback::TICKS_PER_SECOND;
use crate::runtime::spawn_tokio;

const WIDTH: i32 = THUMB_WIDTH as i32;
const HEIGHT: i32 = WIDTH * 9 / 16;
/// Gap between the card and the seek bar, and from the window's edges.
const GAP: i32 = 12;
/// How long the card stays after a seek that isn't a hover (controller).
const FLASH: Duration = Duration::from_millis(1200);

#[derive(Clone)]
pub struct ScrubPreview {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Box,
    picture: gtk::Picture,
    chapter: gtk::Label,
    time: gtk::Label,
    source: RefCell<Option<Source>>,
    /// Decoded frames: BIF frame index, or chapter index for chapter art.
    textures: RefCell<HashMap<usize, gdk::Texture>>,
    /// Which frame the card should show now; late images for others drop.
    wanted: Cell<Option<usize>>,
    /// Bumped per video, so a slow download for the last one is ignored.
    generation: Cell<u64>,
    hide_timer: RefCell<Option<glib::SourceId>>,
}

/// Where this video's pictures come from.
struct Source {
    client: Arc<EmbyClient>,
    item_id: String,
    chapters: Vec<ChapterInfo>,
    thumbnails: Option<Rc<Thumbnails>>,
}

impl ScrubPreview {
    pub fn new() -> Self {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .build();
        let frame = super::fixed_picture(&picture, WIDTH, HEIGHT);
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.add_css_class("scrub-frame");
        let chapter = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .hexpand(true)
            .css_classes(["heading"])
            .build();
        let time = gtk::Label::builder().css_classes(["numeric"]).build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .width_request(WIDTH)
            .can_target(false)
            .visible(false)
            .css_classes(["osd", "scrub-preview"])
            .build();
        root.append(&frame);
        root.append(&chapter);
        root.append(&time);
        ScrubPreview {
            inner: Rc::new(Inner {
                root,
                picture,
                chapter,
                time,
                source: RefCell::new(None),
                textures: RefCell::new(HashMap::new()),
                wanted: Cell::new(None),
                generation: Cell::new(0),
                hide_timer: RefCell::new(None),
            }),
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.inner.root
    }

    /// Forgets the last video (a new one is about to start).
    pub fn clear(&self) {
        let inner = &self.inner;
        inner.generation.set(inner.generation.get() + 1);
        inner.source.replace(None);
        inner.textures.borrow_mut().clear();
        inner.wanted.set(None);
        self.hide();
    }

    /// Gets `item`'s thumbnails (web links have none; chapter art only).
    pub fn load(&self, client: &Arc<EmbyClient>, item: &BaseItem, media_source_id: Option<&str>) {
        self.clear();
        self.inner.source.replace(Some(Source {
            client: client.clone(),
            item_id: item.id.clone(),
            chapters: item.chapters.clone(),
            thumbnails: None,
        }));
        let Some(media_source_id) = media_source_id else {
            return;
        };
        let generation = self.inner.generation.get();
        let (client, id, source) = (client.clone(), item.id.clone(), media_source_id.to_string());
        let weak = Rc::downgrade(&self.inner);
        glib::spawn_future_local(async move {
            let thumbnails = spawn_tokio(async move { client.trickplay(&id, &source).await }).await;
            let Some(inner) = weak.upgrade() else { return };
            if inner.generation.get() != generation {
                return;
            }
            match thumbnails {
                Ok(Some(thumbnails)) => {
                    if let Some(source) = inner.source.borrow_mut().as_mut() {
                        source.thumbnails = Some(Rc::new(thumbnails));
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::debug!("trickplay: {e:#}"),
            }
        });
    }

    /// Shows the card for `seconds`, centred on `x` (in `overlay`), just
    /// above `bar_top` (the seek bar's top edge in `overlay`).
    pub fn show_at(&self, overlay: &gtk::Widget, seconds: f64, x: f64, bar_top: f64) {
        let inner = &self.inner;
        if inner.source.borrow().is_none() {
            return;
        }
        if let Some(timer) = inner.hide_timer.take() {
            timer.remove();
        }
        let ticks = (seconds * TICKS_PER_SECOND as f64) as i64;
        inner.time.set_label(&format_timestamp(ticks));
        let chapter = self.chapter_at(ticks);
        let name = chapter
            .as_ref()
            .and_then(|(_, c)| c.name.clone())
            .filter(|n| !n.is_empty());
        inner.chapter.set_visible(name.is_some());
        inner.chapter.set_label(name.as_deref().unwrap_or_default());
        self.show_picture(seconds, chapter);

        let width = inner.root.width().max(WIDTH + 2 * GAP);
        let max_start = (overlay.width() - width - GAP).max(GAP);
        let start = ((x as i32) - width / 2).clamp(GAP, max_start);
        inner.root.set_margin_start(start);
        inner
            .root
            .set_margin_bottom((overlay.height() - bar_top as i32 + GAP).max(0));
        inner.root.set_visible(true);
    }

    /// Like [`show_at`](Self::show_at), then hides again after a moment.
    pub fn flash_at(&self, overlay: &gtk::Widget, seconds: f64, x: f64, bar_top: f64) {
        self.show_at(overlay, seconds, x, bar_top);
        let weak = Rc::downgrade(&self.inner);
        let timer = glib::timeout_add_local_once(FLASH, move || {
            if let Some(inner) = weak.upgrade() {
                inner.hide_timer.take();
                inner.root.set_visible(false);
            }
        });
        self.inner.hide_timer.replace(Some(timer));
    }

    pub fn hide(&self) {
        if let Some(timer) = self.inner.hide_timer.take() {
            timer.remove();
        }
        self.inner.root.set_visible(false);
    }

    /// The chapter playing at `ticks` (index into the item's chapters).
    fn chapter_at(&self, ticks: i64) -> Option<(usize, ChapterInfo)> {
        let source = self.inner.source.borrow();
        let chapters = &source.as_ref()?.chapters;
        let index = chapters
            .partition_point(|c| c.start_position_ticks <= ticks)
            .checked_sub(1)?;
        Some((index, chapters[index].clone()))
    }

    fn show_picture(&self, seconds: f64, chapter: Option<(usize, ChapterInfo)>) {
        let inner = &self.inner;
        let source = inner.source.borrow();
        let Some(source) = source.as_ref() else {
            return;
        };

        if let Some(thumbnails) = &source.thumbnails {
            let (index, jpeg) = thumbnails.at(seconds);
            // BIF frames and chapter images never mix within one video.
            let texture = inner.textures.borrow().get(&index).cloned();
            let texture = texture.or_else(|| {
                let texture =
                    gdk::Texture::from_bytes(&glib::Bytes::from_owned(jpeg.to_vec())).ok()?;
                inner.textures.borrow_mut().insert(index, texture.clone());
                Some(texture)
            });
            self.set_picture(texture.as_ref());
            return;
        }

        let Some((index, tag)) = chapter.and_then(|(i, c)| Some((i, c.image_tag?))) else {
            self.set_picture(None);
            return;
        };
        inner.wanted.set(Some(index));
        if let Some(texture) = inner.textures.borrow().get(&index).cloned() {
            self.set_picture(Some(&texture));
            return;
        }
        self.set_picture(None);
        let (client, id) = (source.client.clone(), source.item_id.clone());
        let generation = inner.generation.get();
        let weak = Rc::downgrade(inner);
        glib::spawn_future_local(async move {
            let bytes =
                spawn_tokio(async move { client.chapter_image(&id, index, &tag).await }).await;
            let Some(inner) = weak.upgrade() else { return };
            if inner.generation.get() != generation {
                return;
            }
            let texture = match bytes.and_then(|b| {
                gdk::Texture::from_bytes(&glib::Bytes::from_owned(b)).map_err(Into::into)
            }) {
                Ok(texture) => texture,
                Err(e) => {
                    tracing::debug!("chapter image {index}: {e:#}");
                    return;
                }
            };
            inner.textures.borrow_mut().insert(index, texture.clone());
            if inner.wanted.get() == Some(index) {
                ScrubPreview { inner }.set_picture(Some(&texture));
            }
        });
    }

    fn set_picture(&self, texture: Option<&gdk::Texture>) {
        let picture = &self.inner.picture;
        picture.set_paintable(texture);
        // Text only (chapter + time) when there's no picture at all.
        if let Some(frame) = picture.parent().and_then(|p| p.parent()) {
            frame.set_visible(texture.is_some());
        }
    }
}
