# Janis — Feature Specs

The canonical description of user-facing behavior. Every new feature lands here before its task is done (CLAUDE.md rule 8).

## App shell

- **Titlebar** — prism logo mark + JANIS wordmark + "Open source" badge; centered search field. The field filters the Library, Local Files and Radio screens live (`searchQuery` channel). On macOS the native traffic lights overlay the left edge (`titleBarStyle: Overlay`); empty areas drag the window. On Windows/Linux the "Hide title bar" setting drops the native window frame — applied instantly at runtime via `setDecorations` — and the header then carries its own minimize, maximize/restore and close controls.
- **Sidebar** — three sections (Playing / Library / Sources) + Settings pinned at the bottom. Active row: pink→violet wash + accent ring. Navigation goes through `navigateTo()`; routes mirror screen names.
- **Mini player** — persistent bottom bar: current art + title/artist (click → Now Playing), prev/play/next, live mini waveform, EQ button. Playback continues across all screens (task lifecycle independent of UI).
- **Boot** — `+layout.svelte` awaits `get_preferences` + the library before rendering anything, so screens always see hydrated stores. Volume, EQ, playback switches and language persist in `janis.db`.
- **OS drag-and-drop** — dropping audio files anywhere in the window imports them (Tauri drag-drop event; paths go to `import_files`).
- **Error toasts** — failures surface, they never fail silently. A `Toaster` mounted in the shell shows an error toast (click to dismiss, auto-dismisses) whenever playback fails (the engine reports an unplayable/unreachable track), a radio station won't connect, or cover art can't load — the last case is deduplicated by key, so a disconnected source (e.g. a NAS going offline mid-session) raises one toast, not one per track. Messages carry a translation key, not resolved text, so a toast re-renders in the active language. Cover tiles whose art was expected but failed to load show a muted "unavailable" mark rather than the normal gradient, so a failed load reads differently from art that simply doesn't exist.

## Now Playing (`/now-playing`)

- Empty state (nothing loaded): icon + "Add music" CTA → file picker.
- Left column (sticky): album art — embedded cover art when the file has one, else a deterministic prism-gradient tile with title initials — ringed by a slow-spinning conic glow. Below: quality badge ("Hi-Res · FLAC 24/96" — Hi-Res when >16-bit or >48 kHz) and a lime "Lossless" badge for lossless formats. Radio mode shows genre + LIVE badges instead.
- Right column: pink eyebrow ("NOW PLAYING · FROM {ALBUM}" / "LIVE RADIO · {STATION}"), hero title, artist / composer (teal, only when tagged) / album columns. In radio mode the hero shows the track the station is playing, falling back to the station name, with artist and album columns when known and station + genre beneath. The art tile shows real cover art from stations that publish it.
- **Waveform** — live time-domain oscilloscope over the playing signal, taken from the engine's analyser after the EQ and gain, so the trace scales with the volume slider (synthetic animation only when nothing is playing). Progress wash + playhead dot; click seeks (local tracks only). Time row shows elapsed / total ("Live" for radio).
- **Transport** — shuffle (accent when on), prev, gradient pulse play/pause, next, repeat (accent when on); volume rail with pink→teal fill, persisted debounced.
- **Queue card** — three-way segmented toggle: **Up Next** (remaining queue after the current track, wrapping once), **Playlist** (the full play queue, current track highlighted), **Album** (the current track's album from the library grouping). Rows show art, title/artist and duration; clicking a row plays from it. An "EQ" pill in the card header opens the equalizer overlay. The toggle survives navigating away and back.
- **Artist spotlight** (local tracks with library matches) — two columns under the queue card: up to 4 more tracks by the current artist (click plays within the artist's tracks) and up to 4 of their albums (click plays the album).

## Library (`/library`)

- Header: violet eyebrow, title, gradient "Add music" button (multi-file picker).
- Tabs (chips): **Playlists** (roadmap placeholder), **Artists** (grid), **Albums** (grid), **Songs** (numbered rows: art, title/artist, composer, duration; click plays from that row within the filtered list).
- Clicking an album or artist tile browses it rather than playing it: an accent ring marks the selected tile, and the row list below the grid — otherwise "Recently added" — shows that album's tracks (in album running order) or that artist's tracks instead, with a "Back to recently added" link to clear it. Clicking a row plays from that list. Re-clicking the selected tile, or switching tabs, clears the selection back to "Recently added". The active tab and the selection both survive navigating to another screen and back — they live in a store, not the screen (CLAUDE.md rule 12).
- "Recently added" rows (first 8, newest scan first) under the grid tabs when nothing is selected.
- Empty library → empty state with "Add folder" CTA. Titlebar search filters every view.

## Discover (`/discover`)

- Teal eyebrow. Shelves derived from the local library only: "Recently added albums", "Rediscover" (oldest additions), "Artists in your library". Horizontal scroll tiles; click plays the group. Empty library → explanatory empty state.

## Web Radio (`/radio`)

- Ember eyebrow. Two chip rows: country chips ("All" first, then every country present, most-populated first) above genre chips ("All" first, then every genre present sorted by label in the active language). The country chips show the country name only (no flag). Both rows are multi-select — clicking a chip toggles it, so several countries and several genres can be active at once; "All" is a clear that deselects everything in its row, and an empty row means no filter. A station shows when it matches any selected country **and** any selected genre. Selections survive navigating away and back. Station cards: gradient tile, name, genre · kbps, red LIVE dot. Click streams the station through the engine, so the EQ and the live visualiser apply exactly as they do to a local file. The card shows "Connecting…" while the stream buffers, then "Live". Active station gets an ember border. Search filters by station name **and** genre label, within the active countries and genres.
- The catalog ships ~1,300 stations across 22 countries (Europe, the UK, the USA and Brazil) and ~45 genres, baked in as static JSON. It merges the hand-picked flagship streams — the SomaFM channels, the Radio France webradios (FIP, France Musique, Mouv'), Radio Paradise mixes, Radio Swiss, French independents (TSF Jazz, Nova, Meuh, Jazz Radio, FG, OÜI FM, Latina), US public/independent stations (KEXP, KCRW, WFMU, KUSC, WQXR, The Jazz Groove, SmoothJazz.com), international independents (NTS, Rinse FM, dublab, Venice Classic) and electronic specialists (Nightwave Plaza, Nightride FM, Ambient Sleeping Pill, Hirschmilch, Sunshine Live, Bassdrive) — with the most-listened stations per country from the [Radio Browser](https://www.radio-browser.info) community database, plus tag-targeted top-ups for genres the per-country lists leave thin (jazz, funk, blues, rock, pop, classical, bossa nova), deduplicated and genre-mapped at build time. No network call is made to browse; only the chosen stream is opened.

## Library metadata

- The scanner reads tags through `lofty`, which normalises every container onto the same keys — ID3v2.2/2.3/2.4 and ID3v1 in MP3, Vorbis comments in FLAC and OGG, MP4 atoms in M4A, APE — so no format needs special handling. Title, artist, album, album artist, composer, genre, year, track and disc numbers (with totals) and embedded cover art all come from there.
- **A missing track number is recovered from the filename**: `07 - Alive.flac`, `2-11 Reprise.mp3`, `104 Title.m4a`. Only leading digits count and only when a separator follows them, so `1984 - Track.mp3` is not read as track 198 and `99Luftballons.mp3` is not read as track 99. A leading `N-` or `N_` is taken as the disc.
- Albums group by album + **album artist**, so a compilation stays one album instead of splintering per featured artist, and their tracks are ordered by disc, then track, then title. Tracks with no position sort last rather than to the front.
- **Cover art shows everywhere a track or album appears**: Now Playing and the mini player, the queue card and artist spotlight beneath it, the Library album/artist grids and track rows, and the Discover shelves. Only radio cards stay a gradient by design.
- Art is read from every tag block in the file, not just the primary one, preferring the front cover and accepting a back cover over nothing. A track with no picture of its own falls back to the rest of its album, so a rip where only the first file is tagged still shows the sleeve throughout.
- Tiles fetch art only once they scroll into view, and it is cached per track — art crosses IPC as base64, so a large library cannot afford to load it all at once. Until it arrives, and for albums that genuinely have none, the generated prism gradient stands in.
- Now Playing shows the track's position on the record ("Track 4 of 9"), disc when there is more than one, year and genre. Titlebar search matches album artist and genre alongside title, artist, album and composer.

## Playback engine

- **Volume normalization** evens out loudness across the library. A track's gain comes from its ReplayGain tags where it has them; otherwise the engine measures the track against EBU R128 while it plays, and remembers the answer for next time — so the library fills itself in as it is listened to. Only a track heard start to finish is recorded; a seek abandons the measurement rather than storing a partial one. The gain is capped so a boost can never clip, and takes effect exactly as each track reaches the speakers, which matters at a gapless join where two tracks are in flight at once.
- Decoding, the 10-band EQ, the analyser and output all run in Rust. One signal path for every source: file **or web radio stream** → symphonia decode → resample (bypassed when the source already matches the device rate) → ten peaking filters → volume + normalization gain → analyser tap → device. The analyser sees the final signal, so the visualiser scales with the volume slider — that is by design, not a canvas bug.
- Web radio is buffered over HTTP into the same decoder a file uses, so a station gets the EQ and a real visualiser. A dropped connection is reopened automatically, backing off from a second to half a minute across eight attempts before the station is reported as unreachable — servers restart, and falling silent is the wrong answer to something that fixes itself.
- **What a station is playing** comes from one of two sources. Stations whose operator publishes a now-playing endpoint (SomaFM, the FIP and France Musique webradios, Mouv', Radio Paradise — 64 of the curated list) are polled for artist, title, album and cover art; Radio France and Radio Paradise supply the artwork, and Radio France says when the track ends so the next poll lands just after it. Everything else falls back to the ICY metadata carried in the stream, parsed into artist and title where its shape allows. A station with a provider ignores ICY: two feeds disagreeing about timing reads worse than either alone.
- **Track transitions** follow whichever of the two switches applies, checked in this order. Crossfade, when on, overlaps the last four seconds of the outgoing track with the start of the incoming one, faded with an equal-power curve so the perceived loudness does not dip at the midpoint; a seek, a manual skip, or turning the switch off cancels a fade already in progress rather than letting it finish. Otherwise, gapless (on by default) joins the next track directly in the playback ring with no flush, so a continuous-mix album plays with no gap or click; turned off, each track instead flushes to a clean, brief silence before the next one starts, like most other players' default behavior.
- Rust owns the queue and the transport; the frontend mirrors engine state and sends commands. Playback survives a webview reload — the engine replays its state on reconnect, including the queue's track ids and the playing station's id, from which the UI rebuilds what it shows.
- Pressing Play after a queue has run its course starts the last track again rather than doing nothing, and switching the output device mid-track continues playback on the new device at its own sample rate and channel layout.
- **Shuffle** walks a real permutation of the queue, so every track is heard once before any repeats, and reshuffles when a repeating queue wraps.
- The visualiser is fed by the engine at ~60 Hz as a compact binary frame (160 waveform points + 10 band magnitudes).

## Local Files (`/local`)

- Lime eyebrow. Dashed drop zone: click opens the file picker; OS drag-and-drop works window-wide. Formats: MP3, FLAC, WAV, AAC/M4A, OGG/Vorbis, Opus, AIFF — all decoded in Rust. Opus is mono or stereo; multichannel Opus is not supported.
- "Scanned library · N files" header with the last scan report (added/updated/skipped), "Add folder" and "Rescan" ghost buttons.
- Table: title, artist, lime format chip, duration. Click plays from the filtered list.
- Watched folders list: path, track count, per-folder Remove (cascade-deletes its tracks). Rescan re-walks all folders and prunes files that vanished — but only under roots that are still reachable, so rescanning with an external drive ejected leaves that drive's library (and its accumulated loudness measurements) intact.

## Spotify (`/spotify`)

- Connect screen (green mark, pitch, feature bullets). The CTA is honest: clicking shows "integration is planned — not available yet". No fake account linking.

## Settings (`/settings`)

- Violet eyebrow. **Equalizer presets** chips (apply immediately + persist) and a "fine-tune" link opening the EQ sheet.
- **Playback** toggles: normalization, gapless and crossfade are all live in the engine, on by default; exclusive output is still a persisted preference the engine does not act on yet. Toggling normalization is heard immediately, mid-track. Gapless and crossfade take effect from the next track boundary (crossfade turned off mid-fade stops it immediately instead of letting it finish).
- **Window** toggle (Windows/Linux only — hidden on macOS, which keeps its overlay title bar): "Hide title bar" drops the native window frame instantly and persists; the app header then shows its own window controls.
- **Audio output** cards: the output device the engine opened and its sample rate (both populate once something has played; "System default" / "—" until then).
- **Language** chips: English / Français. Persisted; `lang` attribute + localStorage FOUC mirror update immediately.
- Open-source banner (GPL-3.0).

## 10-band Equalizer (overlay)

- Bottom sheet over everything (scrim click / Escape / × closes). Preset chips + Reset. Ten vertical bands (32 Hz–16 kHz), ±12 dB in 0.5 dB steps, drag anywhere on the column; value readout colored lime (boost) / ember (cut). Gains apply live to the engine's filters and persist (debounced) with the preset name; hand-moving a band flips the preset to "Custom".

## Persistence (`janis.db`, app-data dir)

- `user_preferences` (single row): volume, EQ gains + preset, four playback switches, language.
- `watched_folders`: path, added_at. `tracks`: path (unique), folder_id (NULL = ad-hoc import), tags (title/artist/album/composer/album artist/genre/year, track and disc numbers with their totals), duration, format, sample rate, bit depth, channels, lossless, added_at, the four ReplayGain tag columns (track/album gain and peak) and the measured loudness pair (`loudness_lufs`, `loudness_peak`) the normalization feature fills in as tracks are heard. Upsert by path — rescans never duplicate, and the measured loudness columns live outside the upsert so a rescan cannot wipe them.
- Schema changes are migrations stamped in SQLite's `user_version`, so an existing library gains new columns instead of silently missing them.
- The webview has no filesystem access at all: the engine reads audio files directly and cover art crosses IPC as base64, so Tauri's asset protocol is not enabled.
