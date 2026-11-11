//! Cover art on its way to the window: the art of whatever plays, and the
//! thumbnails of lists and grids. Reading and decoding run on the picture
//! worker; a result that arrives after the listener moved on is dropped.

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image};

use super::{post, Application};
use crate::covers::{self, Pixels};
use crate::user_interface::Covers;

/// The art of whatever is playing.
#[derive(Default)]
pub(super) struct CurrentArt {
    /// Bumped on every change of track, so a slow load that finishes after
    /// the listener moved on is recognised and dropped.
    pub(super) epoch: u64,
    /// The track whose cover is loaded or loading.
    track_id: Option<i64>,
    pub(super) track_cover: Option<Image>,
    pub(super) track_cover_failed: bool,
    /// Bumped each time a station announces other art, or none, for the same
    /// reason.
    pub(super) stream_epoch: u64,
    pub(super) stream_cover: Option<Image>,
}

/// A cover for a list or grid tile.
pub(super) enum Thumbnail {
    Loading,
    Ready(Image),
    /// Neither the file nor its album carries art.
    Absent,
    /// Art exists but could not be read.
    Failed,
}

impl Application {
    /// Wires the `Covers` global: tiles ask it for their thumbnail.
    pub(super) fn connect_covers(self: &Rc<Self>) {
        let covers = self.window.global::<Covers>();
        let weak = Rc::downgrade(self);
        covers.on_thumbnail(move |track_id, _revision| {
            weak.upgrade()
                .map(|application| application.thumbnail(i64::from(track_id)))
                .unwrap_or_default()
        });
        let weak = Rc::downgrade(self);
        covers.on_unavailable(move |track_id, _revision| {
            weak.upgrade().is_some_and(|application| {
                matches!(
                    application.thumbnails.borrow().get(i64::from(track_id)),
                    Some(Thumbnail::Failed)
                )
            })
        });
    }

    pub(super) fn load_current_cover(&self, track_id: Option<i64>) {
        let epoch = {
            let mut art = self.current_art.borrow_mut();
            // The engine confirms a track the click already showed: its
            // cover is not read a second time.
            if track_id.is_some() && art.track_id == track_id && !art.track_cover_failed {
                return;
            }
            art.track_id = track_id;
            art.epoch += 1;
            art.track_cover = None;
            art.track_cover_failed = false;
            art.epoch
        };
        let Some(track_id) = track_id else {
            return;
        };
        let database = Arc::clone(&self.database);
        self.picture_worker.run(move || {
            let loaded = covers::track_cover(&database, track_id, covers::HERO_SIDE);
            post(move |application| application.current_cover_loaded(epoch, loaded));
        });
    }

    pub(super) fn current_cover_loaded(&self, epoch: u64, loaded: Result<Option<Pixels>, String>) {
        {
            let mut art = self.current_art.borrow_mut();
            if art.epoch != epoch {
                return;
            }
            match loaded {
                Ok(pixels) => art.track_cover = pixels.map(Image::from_rgba8),
                Err(error) => {
                    log::error!("current cover: {error}");
                    art.track_cover_failed = true;
                }
            }
        }
        if self.current_art.borrow().track_cover_failed {
            self.notify_cover_unavailable();
        }
        self.refresh_player();
    }

    pub(super) fn show_stream_cover(&self, url: Option<String>) {
        let epoch = {
            let mut art = self.current_art.borrow_mut();
            art.stream_epoch += 1;
            art.stream_epoch
        };
        let Some(url) = url else {
            self.current_art.borrow_mut().stream_cover = None;
            return;
        };
        self.picture_worker.run(move || {
            let decoded = covers::data_url_cover(&url, covers::HERO_SIDE);
            post(move |application| application.stream_cover_loaded(epoch, decoded));
        });
    }

    pub(super) fn stream_cover_loaded(&self, epoch: u64, decoded: Result<Pixels, String>) {
        if self.current_art.borrow().stream_epoch != epoch {
            return;
        }
        match decoded {
            Ok(pixels) => {
                self.current_art.borrow_mut().stream_cover = Some(Image::from_rgba8(pixels));
            }
            Err(error) => {
                log::error!("station cover: {error}");
                // The art announced before belongs to what played before.
                self.current_art.borrow_mut().stream_cover = None;
                self.notify_cover_unavailable();
            }
        }
        self.refresh_player();
    }

    /// The cover of a library track for a list or grid tile. Asking for one
    /// that is not cached starts its load; the tile is told when it lands.
    pub(super) fn thumbnail(&self, track_id: i64) -> Image {
        if track_id <= 0 {
            return Image::default();
        }
        if let Some(thumbnail) = self.thumbnails.borrow().get(track_id) {
            return match thumbnail {
                Thumbnail::Ready(image) => image.clone(),
                Thumbnail::Loading | Thumbnail::Absent | Thumbnail::Failed => Image::default(),
            };
        }
        self.thumbnails
            .borrow_mut()
            .insert(track_id, Thumbnail::Loading);
        let database = Arc::clone(&self.database);
        let epoch = self.thumbnail_epoch.get();
        self.picture_worker.run(move || {
            let loaded = covers::track_cover(&database, track_id, covers::THUMBNAIL_SIDE);
            post(move |application| application.thumbnail_loaded(epoch, track_id, loaded));
        });
        Image::default()
    }

    pub(super) fn thumbnail_loaded(
        &self,
        epoch: u64,
        track_id: i64,
        loaded: Result<Option<Pixels>, String>,
    ) {
        if self.thumbnail_epoch.get() != epoch {
            return;
        }
        let thumbnail = match loaded {
            Ok(Some(pixels)) => Thumbnail::Ready(Image::from_rgba8(pixels)),
            Ok(None) => Thumbnail::Absent,
            Err(error) => {
                log::error!("cover of track {track_id}: {error}");
                self.notify_cover_unavailable();
                Thumbnail::Failed
            }
        };
        self.thumbnails.borrow_mut().insert(track_id, thumbnail);
        self.bump_cover_revision();
    }

    /// Tells every cover tile to look its art up again.
    pub(super) fn bump_cover_revision(&self) {
        let covers = self.window.global::<Covers>();
        covers.set_revision(covers.get_revision().wrapping_add(1));
    }

    pub(super) fn notify_cover_unavailable(&self) {
        self.notify_error("error.cover", Vec::new(), Some("cover-unavailable"));
    }
}
