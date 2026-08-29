# Janis — Feature Specs

The canonical description of user-facing behavior. Every new feature lands here before its task is done (CLAUDE.md rule 8).

## App shell

- **Titlebar** — prism logo mark + JANIS wordmark + "Open source" badge; centered search field. The field filters the Library, Local Files and Radio screens live (`searchQuery` channel). On macOS the native traffic lights overlay the left edge (`titleBarStyle: Overlay`); empty areas drag the window.
- **Sidebar** — three sections (Playing / Library / Sources) + Settings pinned at the bottom. Active row: pink→violet wash + accent ring. Navigation goes through `navigateTo()`; routes mirror screen names.
- **Mini player** — persistent bottom bar: current art + title/artist (click → Now Playing), prev/play/next, live mini waveform, EQ button. Playback continues across all screens (task lifecycle independent of UI).
- **Boot** — `+layout.svelte` awaits `get_preferences` + the library before rendering anything, so screens always see hydrated stores. Volume, EQ, playback switches and language persist in `janis.db`.
- **OS drag-and-drop** — dropping audio files anywhere in the window imports them (Tauri drag-drop event; paths go to `import_files`).

## Now Playing (`/now-playing`)

- Empty state (nothing loaded): icon + "Add music" CTA → file picker.
- Left column (sticky): album art — embedded cover art when the file has one, else a deterministic prism-gradient tile with title initials — ringed by a slow-spinning conic glow. Below: quality badge ("Hi-Res · FLAC 24/96" — Hi-Res when >16-bit or >48 kHz) and a lime "Lossless" badge for lossless formats. Radio mode shows genre + LIVE badges instead.
- Right column: pink eyebrow ("NOW PLAYING · FROM {ALBUM}" / "LIVE RADIO · {STATION}"), hero title, artist / composer (teal, only when tagged) / album columns. In radio mode the hero shows the track the station announces over ICY metadata when it sends one, falling back to the station name, with station + genre columns beneath.
- **Waveform** — live time-domain oscilloscope over the playing signal, taken from the engine's analyser after the EQ (synthetic animation only when nothing is playing). Progress wash + playhead dot; click seeks (local tracks only). Time row shows elapsed / total ("Live" for radio).
- **Transport** — shuffle (accent when on), prev, gradient pulse play/pause, next, repeat (accent when on); volume rail with pink→teal fill, persisted debounced.
- **Queue card** — three-way segmented toggle: **Up Next** (remaining queue after the current track, wrapping once), **Playlist** (the full play queue, current track highlighted), **Album** (the current track's album from the library grouping). Rows show art, title/artist and duration; clicking a row plays from it. An "EQ" pill in the card header opens the equalizer overlay.
- **Artist spotlight** (local tracks with library matches) — two columns under the queue card: up to 4 more tracks by the current artist (click plays within the artist's tracks) and up to 4 of their albums (click plays the album).

## Library (`/library`)

- Header: violet eyebrow, title, gradient "Add music" button (multi-file picker).
- Tabs (chips): **Playlists** (roadmap placeholder), **Artists** (grid, click plays the artist's tracks), **Albums** (grid, click plays the album), **Songs** (numbered rows: art, title/artist, composer, duration; click plays from that row within the filtered list).
- "Recently added" rows (first 8) under the grid tabs.
- Empty library → empty state with "Add folder" CTA. Titlebar search filters every view.

## Discover (`/discover`)

- Teal eyebrow. Shelves derived from the local library only: "Recently added albums", "Rediscover" (oldest additions), "Artists in your library". Horizontal scroll tiles; click plays the group. Empty library → explanatory empty state.

## Web Radio (`/radio`)

- Ember eyebrow. Genre chips ("All" first, then every genre present in the curated list sorted by label in the active language). Station cards: gradient tile, name, genre · kbps, red LIVE dot. Click streams the station through the engine, so the EQ and the live visualiser apply exactly as they do to a local file. The card shows "Connecting…" while the stream buffers, then "Live". Active station gets an ember border. Search filters by station name **and** genre label.
- The curated list ships ~100 probe-verified HTTPS streams across ~35 genres: the SomaFM channels, the Radio France webradios (FIP, France Musique, Mouv'), Radio Paradise mixes, Radio Swiss, French independents (TSF Jazz, Nova, Meuh, Jazz Radio, FG, OÜI FM, Latina), US public/independent stations (KEXP, KCRW, WFMU, KUSC, WQXR, The Jazz Groove, SmoothJazz.com), international independents (NTS, Rinse FM, dublab, Venice Classic) and electronic specialists (Nightwave Plaza, Nightride FM, Ambient Sleeping Pill, Hirschmilch, Sunshine Live, Bassdrive).

## Playback engine

- Decoding, the 10-band EQ, the analyser and output all run in Rust. One signal path for every source: file **or web radio stream** → symphonia decode → resample (bypassed when the source already matches the device rate) → ten peaking filters → analyser tap → volume → device.
- Web radio is buffered over HTTP into the same decoder a file uses, so a station gets the EQ and a real visualiser. In-band ICY metadata is parsed out and surfaced as the current track.
- Rust owns the queue and the transport; the frontend mirrors engine state and sends commands. Playback survives a webview reload — the engine replays its state on reconnect.
- **Shuffle** walks a real permutation of the queue, so every track is heard once before any repeats, and reshuffles when a repeating queue wraps.
- The visualiser is fed by the engine at ~60 Hz as a compact binary frame (160 waveform points + 10 band magnitudes).

## Local Files (`/local`)

- Lime eyebrow. Dashed drop zone: click opens the file picker; OS drag-and-drop works window-wide. Formats: MP3, FLAC, WAV, AAC/M4A, OGG/Opus, AIFF.
- "Scanned library · N files" header with the last scan report (added/updated/skipped), "Add folder" and "Rescan" ghost buttons.
- Table: title, artist, lime format chip, duration. Click plays from the filtered list.
- Watched folders list: path, track count, per-folder Remove (cascade-deletes its tracks). Rescan re-walks all folders and prunes files that vanished.

## Spotify (`/spotify`)

- Connect screen (green mark, pitch, feature bullets). The CTA is honest: clicking shows "integration is planned — not available yet". No fake account linking.

## Settings (`/settings`)

- Violet eyebrow. **Equalizer presets** chips (apply immediately + persist) and a "fine-tune" link opening the EQ sheet.
- **Playback** toggles: gapless, crossfade, normalization, exclusive output. Persisted preferences; the engine wires them up progressively (they do not all alter playback yet — descriptions stay factual).
- **Audio output** cards: the output device the engine opened and its sample rate (both populate once something has played; "System default" / "—" until then).
- **Language** chips: English / Français. Persisted; `lang` attribute + localStorage FOUC mirror update immediately.
- Open-source banner (GPL-3.0).

## 10-band Equalizer (overlay)

- Bottom sheet over everything (scrim click / Escape / × closes). Preset chips + Reset. Ten vertical bands (32 Hz–16 kHz), ±12 dB in 0.5 dB steps, drag anywhere on the column; value readout colored lime (boost) / ember (cut). Gains apply live to the engine's filters and persist (debounced) with the preset name; hand-moving a band flips the preset to "Custom".

## Persistence (`janis.db`, app-data dir)

- `user_preferences` (single row): volume, EQ gains + preset, four playback switches, language.
- `watched_folders`: path, added_at. `tracks`: path (unique), folder_id (NULL = ad-hoc import), tags (title/artist/album/composer), duration, format, sample rate, bit depth, channels, lossless, added_at. Upsert by path — rescans never duplicate.
- The webview has no filesystem access at all: the engine reads audio files directly and cover art crosses IPC as base64, so Tauri's asset protocol is not enabled.
