//! Cover art on its way to the window: the art of whatever plays, and the
//! thumbnails of lists and grids. Reading — from a file, or from a media
//! server — and decoding run on the picture worker for the art of whatever
//! plays, and on the thumbnail workers for the tiles; a result that arrives
//! after the listener moved on is dropped. Thumbnails are cached by cover
//! key, so the songs of a server album load their shared picture once, and
//! a thumbnail loads only once its tile stayed in view across a cover check
//! (`cover_requests`), so a fast scroll does not queue the tiles it flew past.
//! Once a long list has rested, the covers just beyond what it draws are
//! prefetched, in the loaders the tiles in view leave free.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Image, Model, TimerMode};

use super::{post, with_application, Application};
use crate::cover_requests::{self, Direction};
use crate::covers::{self, CoverKey, CoverRequest, CurrentArtRequests, Pixels};
use crate::jellyfin::{self, Session};
use crate::library::{self, CoverSource};
use crate::persistence::DbState;
use crate::user_interface::{CoverList, Covers};

/// How often the tiles in view are asked to repeat their requests while
/// some wait. A tile must stay in view this long, and at most twice this,
/// before its cover starts loading.
const COVER_CHECK_INTERVAL: Duration = Duration::from_millis(100);
/// Thumbnails loading at once: enough to fill a grid quickly from a media
/// server, few enough that a slow one does not get a flood of requests.
pub(super) const PARALLEL_THUMBNAILS: usize = 4;
/// Of those, the most that prefetches may take, so a tile coming into view
/// always finds a loader free.
pub(super) const PARALLEL_PREFETCHES: usize = 2;
/// How long a list must rest before the covers beyond what it draws are
/// prefetched: a scroll reports a new window every frame.
const PREFETCH_SETTLE: Duration = Duration::from_millis(250);

/// The places a long list draws, as it reports them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ListWindow {
    list: CoverList,
    first: usize,
    count: usize,
}

/// The window last reported, and the one the list last rested on, which
/// tells which way it moved since.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ListWindows {
    reported: Option<ListWindow>,
    rested: Option<ListWindow>,
}

/// The art of whatever is playing.
#[derive(Default)]
pub(super) struct CurrentArt {
    /// Which loads are still wanted, and whether the track's cover failed.
    pub(super) requests: CurrentArtRequests,
    pub(super) track_cover: Option<Image>,
    pub(super) stream_cover: Option<Image>,
}

/// A cover for a list or grid tile.
pub(super) enum Thumbnail {
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
                let key = application.cover_keys.borrow().key(i64::from(track_id));
                matches!(
                    application.thumbnails.borrow().get(&key),
                    Some(Thumbnail::Failed)
                )
            })
        });
        let weak = Rc::downgrade(self);
        covers.on_in_view(move |list, first, count| {
            if let Some(application) = weak.upgrade() {
                application.list_in_view(list, first, count);
            }
        });
    }

    /// A long list reports what it draws, every frame of a scroll: its
    /// nearby covers are prefetched once it rests.
    fn list_in_view(&self, list: CoverList, first: i32, count: i32) {
        let mut windows = self.list_windows.get();
        windows.reported = Some(ListWindow {
            list,
            first: usize::try_from(first).unwrap_or(0),
            count: usize::try_from(count).unwrap_or(0),
        });
        self.list_windows.set(windows);
        self.prefetch_timer
            .start(TimerMode::SingleShot, PREFETCH_SETTLE, || {
                with_application(Application::prefetch_nearby);
            });
    }

    /// Queues the covers just beyond the window the list rested on, the side
    /// it was heading for first, in place of those of its last resting place.
    fn prefetch_nearby(&self) {
        let mut windows = self.list_windows.get();
        let Some(window) = windows.reported else {
            return;
        };
        let direction = match windows.rested {
            Some(rested) if rested.list == window.list && rested.first > window.first => {
                Direction::Up
            }
            _ => Direction::Down,
        };
        windows.rested = Some(window);
        self.list_windows.set(windows);

        let length = self.list_length(window.list);
        let keys: Vec<CoverKey> = {
            let cover_keys = self.cover_keys.borrow();
            let thumbnails = self.thumbnails.borrow();
            let mut keys: Vec<CoverKey> = Vec::new();
            for place in cover_requests::nearby(window.first, window.count, length, direction) {
                let Some(track_id) = self
                    .list_track_id(window.list, place)
                    .filter(|track_id| *track_id > 0)
                else {
                    continue;
                };
                let key = cover_keys.key(i64::from(track_id));
                if thumbnails.get(&key).is_none() && !keys.contains(&key) {
                    keys.push(key);
                }
            }
            keys
        };
        let started = {
            let mut requests = self.cover_requests.borrow_mut();
            requests.prefetch(keys);
            requests.start()
        };
        self.load_thumbnails(started);
    }

    /// How many places a long list has.
    fn list_length(&self, list: CoverList) -> usize {
        match list {
            CoverList::Albums => self.library_albums.row_count(),
            CoverList::Artists => self.library_artists.row_count(),
            CoverList::Songs => self.library_songs.row_count(),
            CoverList::None => 0,
        }
    }

    /// The track whose cover a place of a long list shows.
    fn list_track_id(&self, list: CoverList, place: usize) -> Option<i32> {
        match list {
            CoverList::Albums => self
                .library_albums
                .row_data(place)
                .map(|card| card.track_id),
            CoverList::Artists => self
                .library_artists
                .row_data(place)
                .map(|card| card.track_id),
            CoverList::Songs => self
                .library_songs
                .row_data(place)
                .map(|track| track.track_id),
            CoverList::None => None,
        }
    }

    pub(super) fn load_current_cover(&self, track_id: Option<i64>) {
        let load = {
            let mut art = self.current_art.borrow_mut();
            let CoverRequest::Cleared { load } = art.requests.request(track_id) else {
                return;
            };
            art.track_cover = None;
            load
        };
        let (Some(epoch), Some(track_id)) = (load, track_id) else {
            return;
        };
        let database = Arc::clone(&self.database);
        let session = self.media_session();
        self.picture_worker.run(move || {
            let loaded = library_cover(&database, session.as_ref(), track_id, covers::HERO_SIDE);
            post(move |application| application.current_cover_loaded(epoch, loaded));
        });
    }

    pub(super) fn current_cover_loaded(&self, epoch: u64, loaded: Result<Option<Pixels>, String>) {
        let failed = {
            let mut art = self.current_art.borrow_mut();
            if !art.requests.loaded(epoch, loaded.is_ok()) {
                return;
            }
            match loaded {
                Ok(pixels) => art.track_cover = pixels.map(Image::from_rgba8),
                Err(error) => log::error!("current cover: {error}"),
            }
            art.requests.failed()
        };
        if failed {
            self.notify_cover_unavailable();
        }
        self.refresh_player();
    }

    pub(super) fn show_stream_cover(&self, url: Option<String>) {
        let epoch = self.current_art.borrow_mut().requests.request_stream();
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
        if !self.current_art.borrow().requests.stream_is_current(epoch) {
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
    /// that is not cached requests it; the tile is told when it lands.
    pub(super) fn thumbnail(&self, track_id: i64) -> Image {
        if track_id <= 0 {
            return Image::default();
        }
        let key = self.cover_keys.borrow().key(track_id);
        if let Some(thumbnail) = self.thumbnails.borrow().get(&key) {
            return match thumbnail {
                Thumbnail::Ready(image) => image.clone(),
                Thumbnail::Absent | Thumbnail::Failed => Image::default(),
            };
        }
        self.cover_requests.borrow_mut().ask(key);
        if !self.cover_timer.running() {
            self.cover_timer
                .start(TimerMode::Repeated, COVER_CHECK_INTERVAL, || {
                    with_application(Application::check_covers);
                });
        }
        Image::default()
    }

    /// The periodic cover check: starts the requests the tiles in view
    /// confirmed, then has them repeat the others, until none waits.
    fn check_covers(&self) {
        let started = self.cover_requests.borrow_mut().check();
        self.load_thumbnails(started);
        if self.cover_requests.borrow().has_waiting() {
            self.bump_cover_revision();
        } else {
            self.cover_timer.stop();
        }
    }

    fn load_thumbnails(&self, keys: Vec<CoverKey>) {
        if keys.is_empty() {
            return;
        }
        let session = self.media_session();
        let epoch = self.cover_requests.borrow().epoch();
        for key in keys {
            let database = Arc::clone(&self.database);
            let session = session.clone();
            self.thumbnail_workers.run(move || {
                let loaded = keyed_cover(&database, session.as_ref(), &key, covers::THUMBNAIL_SIDE);
                post(move |application| application.thumbnail_loaded(epoch, key, loaded));
            });
        }
    }

    pub(super) fn thumbnail_loaded(
        &self,
        epoch: u64,
        key: CoverKey,
        loaded: Result<Option<Pixels>, String>,
    ) {
        let (shown, started) = {
            let mut requests = self.cover_requests.borrow_mut();
            // A load from before the library was replaced is dropped.
            let Some(shown) = requests.finished(&key, epoch) else {
                return;
            };
            (shown, requests.start())
        };
        self.load_thumbnails(started);
        let thumbnail = match loaded {
            Ok(Some(pixels)) => Thumbnail::Ready(Image::from_rgba8(pixels)),
            Ok(None) => Thumbnail::Absent,
            Err(error) => {
                log::error!("cover {key:?}: {error}");
                self.notify_cover_unavailable();
                Thumbnail::Failed
            }
        };
        self.thumbnails.borrow_mut().insert(key, thumbnail);
        // A prefetched cover no tile shows yet changes nothing on screen.
        if shown {
            self.bump_cover_revision();
        }
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

/// The cover a key names: a track's, found as `library_cover` finds it, or a
/// server picture, asked for directly.
fn keyed_cover(
    database: &DbState,
    session: Option<&Session>,
    key: &CoverKey,
    longest_side: u32,
) -> Result<Option<Pixels>, String> {
    match key {
        CoverKey::Track(track_id) => library_cover(database, session, *track_id, longest_side),
        CoverKey::ServerPicture(item) => server_picture(session, item, longest_side),
    }
}

/// A library track's cover: read from its file (or its album's), or asked of
/// its server. `Ok(None)` when neither has art; `Err` when art exists but
/// could not be had — a server that stopped answering, among others.
fn library_cover(
    database: &DbState,
    session: Option<&Session>,
    track_id: i64,
    longest_side: u32,
) -> Result<Option<Pixels>, String> {
    match library::cover_source(database, track_id) {
        CoverSource::File => library::track_cover(database, track_id)
            .map(|art| covers::base64_cover(&art.data_base64, longest_side))
            .transpose(),
        CoverSource::Server { artwork_item: None } => Ok(None),
        CoverSource::Server {
            artwork_item: Some(item),
        } => server_picture(session, &item, longest_side),
    }
}

/// A server item's picture, scaled by the server to `longest_side`. Nothing
/// without a session: the songs leave the library when it ends.
fn server_picture(
    session: Option<&Session>,
    item: &str,
    longest_side: u32,
) -> Result<Option<Pixels>, String> {
    let Some(session) = session else {
        return Ok(None);
    };
    let bytes = jellyfin::artwork(session, item, longest_side)
        .map_err(|failure| format!("server picture {item}: {failure:?}"))?;
    covers::picture_cover(bytes, longest_side).map(Some)
}
