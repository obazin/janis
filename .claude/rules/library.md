---
paths:
  - "src/library.rs"
  - "src/persistence.rs"
  - "src/library_view.rs"
  - "src/covers.rs"
  - "src/application/library_screens.rs"
---

# The library and the database

- **One store.** `janis.db` (SQLite, bundled) in the per-user data directory holds the preferences row and the track library, behind one mutex-guarded connection (`DbState`). SQL lives only in `library.rs` and `persistence.rs`.
- **The lock is never held across slow work.** The engine thread takes the same mutex for its loudness reads and writes: a scan walks and probes files unlocked and writes in bursts of 64 inside one transaction (`scan_folder`, `flush_batch`). Keep that shape in anything that touches many files.
- **Paths are stored as text.** A file whose path is not valid Unicode is skipped and counted, never stored altered.
- **The walk follows links** and counts what it cannot enter (an unreadable folder, a link back up the tree, a dangling link) as skipped; nothing is dropped without a count.
- **Scans never run on the interface thread.** `add_watched_folder`, `import_files`, `rescan_library` and `remove_watched_folder` go through `Application::scan`, which runs the change on the library worker and comes back with a fresh `LibrarySnapshot`.
- **Schema changes are migrations.** `SCHEMA` is frozen at version 1. A change is a new entry appended to `MIGRATIONS`, never an edit to an applied one, with its version stamp inside the migration's transaction; bump `SCHEMA_VERSION` and add a migration test.
- **Pruning is guarded.** A track is only removed when its watched folder (or, for an ad-hoc import, its parent directory) is reachable and the file's own check answers "not found" (`Path::try_exists`), never on another error. Never weaken that: an ejected drive or a folder that denies access must not empty the library.
- **A snapshot is one state of the library.** `LibrarySnapshot::read` reads every part under one lock with the library's revision; a write that changes what it reads takes `DbState::lock_for_library_change`, which moves the revision on (preferences and loudness use `lock`). `Application::take_library` reads again instead of taking a snapshot that a write has overtaken.
- **Loudness is written outside the upsert**, so a rescan does not wipe a measurement. A background measurement names the row and the path it read (`PendingLoudness`) and is stored only on a row still holding that path: SQLite reuses a removed row's id.
- **Grouping and search are pure** (`library_view.rs`): albums by album + album artist, artists by track artist, the search fields of each screen. The controller keeps the grouped library and what each screen currently shows.
- **Covers.** A tile asks the `Covers` global for its thumbnail when it appears; the controller decodes on the picture worker into a `BoundedCache` of 300 entries, keyed by `CoverKey` (a server picture is shared by every song that shows it), and bumps the revision. A list that can show more tiles than the cache holds must be windowed, or eviction and reload chase each other. The cache is cleared when the library is replaced.
- **The data directory** is the platform's per-user data directory under `com.olivierbazin.janis` (`platform::data_directory`). Do not move it: existing libraries live there.
- **A media server's songs are rows too.** Their `path` is `jellyfin://<server>/<item>`, their `server_id` points at `media_servers` (cascade delete) and `artwork_item` names their picture. `sync_server_tracks` writes them in bursts like a scan and removes the server's rows a complete listing no longer holds; it also keeps the server's libraries (`server_libraries`, whose `shown` survives a sync) and files each song in them (`track_libraries`). Hiding a library never deletes rows: the controller keeps every track in `all_tracks` and builds the screens from `library_view::shown_tracks`. The file scanner, the pruner and the file cover lookup skip them (`server_id IS NULL`): never stat or open a server row as a file.
- **Favourite songs** are rows of `favourite_tracks`, cascading from `tracks`, ordered by `added_at` then their own `id`. The scanner's upsert never touches them; `sync_server_tracks` stars or unstars each server song as its listing says, in the song's savepoint. `FavouriteTracks` holds the stars in the controller and is restored from every `LibrarySnapshot`; a star still on its way to the server wins over the snapshot until its push answers.
- **Playlists** are rows of `playlists` (a server's carry `server_id`, cascading, and its `server_item`) holding `playlist_entries` in `position` order, a song at most once, cascading from the playlist and the song; `playlist_groups` and the chosen `picture` are Janis's own. `sync_server_playlists` runs after a server's songs, only on a listing that read the playlists whole: it upserts each by `(server_id, server_item)`, so the group and picture survive, replaces its entries with the listed songs Janis holds, and removes the server's playlists no longer listed. `Playlists` is restored from every `LibrarySnapshot`; a change still on its way to the server wins until its push answers.
