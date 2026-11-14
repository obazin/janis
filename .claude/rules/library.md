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
- **Pruning is guarded.** A track is only removed when its watched folder (or, for an ad-hoc import, its parent directory) is reachable and the file is not in it. Never weaken that: an ejected drive must not empty the library.
- **Loudness is written outside the upsert**, so a rescan does not wipe a measurement.
- **Grouping and search are pure** (`library_view.rs`): albums by album + album artist, artists by track artist, the search fields of each screen. The controller keeps the grouped library and what each screen currently shows.
- **Covers.** A tile asks the `Covers` global for its thumbnail when it appears; the controller decodes on the picture worker into a `BoundedCache` of 300 entries, keyed by `CoverKey` (a server picture is shared by every song that shows it), and bumps the revision. A list that can show more tiles than the cache holds must be windowed, or eviction and reload chase each other. The cache is cleared when the library is replaced.
- **The data directory** is the platform's per-user data directory under `com.olivierbazin.janis` (`platform::data_directory`). Do not move it: existing libraries live there.
- **A media server's songs are rows too.** Their `path` is `jellyfin://<server>/<item>`, their `server_id` points at `media_servers` (cascade delete) and `artwork_item` names their picture. `sync_server_tracks` writes them in bursts like a scan and removes the server's rows a complete listing no longer holds. The file scanner, the pruner and the file cover lookup skip them (`server_id IS NULL`): never stat or open a server row as a file.
