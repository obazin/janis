# Decisions log

Design and technical decisions for Janis, oldest first within each section. Each entry says what was decided, why, and whether it still holds. Superseded entries point to what replaced them; the code, the README and the specifications only describe current decisions.

All entries date from 2026-11-11 unless stated. Entries marked "first version" were made between 2026-08-26 and 2026-09 and carried through the rewrite unchanged.

## Stack and platform

**D1 · A native application: Rust throughout, with a Slint interface** — *current*
One process and one language. The interface is Slint 1.18 on winit, compiled from `ui/app.slint`; everything else is one Rust crate. Measured against the previous build on the same machine, playing a local track: about 9–14 % of a processor core against 30 %, and 240–305 MB against 352 MB; idle with the window visible, 0.2 % and 136 MB against 25 % and 276 MB. The costs are a larger download (30 MB bundle against 23 MB, because the renderer, the text stack and the fonts ship in the binary where the webview was the system's), no backdrop blur, and an installer pipeline to maintain.
Superseded: Tauri v2 with a Svelte 5 / SvelteKit 2 / Tailwind v4 webview frontend (D1a, first version). Its Rust backend was already thin and Tauri-free in substance, which is what made the rewrite a port rather than a redesign.

**D2 · The whole audio path lives in the `audio-stack-rs` crate** — *current* (first version)
Decoding, the EQ, the analyser, output-device management, the transport, web radio and audio-file metadata are a standalone MIT crate, consumed as a git dependency pinned to a tag and reached only through its facade, which `src/audio.rs` re-exports. Janis injects two traits (`EventSink`, `Store`). No audio or metadata crate is a direct dependency. The crate has no Tauri reference, so the rewrite did not touch it.

**D3 · FemtoVG renderer; Skia measured and set aside** — *current*
FemtoVG (OpenGL) on the winit backend. Skia was tried for its partial repaint: on the same screens it used no less of the graphics processor (8.8–13.7 %, and more with partial repaint on Now Playing), three times the processor time (25–30 %), and added about 10 MB to the binary.

**D4 · One dark theme, routed through tokens** — *current* (first version)
Janis ships one "prism" theme. Every colour, typeface, text size, radius, shadow and duration is a token in `ui/tokens.slint`, so a second theme would only be other values there. The tokens carry the sRGB values of the first version's OKLCH palette.

**D5 · Cross-platform, with what differs in one module** — *current*
macOS, Windows and Linux. `src/platform.rs` holds what differs behind neutral names: the data directory, the title bar the application draws itself where the system keeps its window buttons over it, window dragging, bringing the window forward, files dropped on the window. While the system moves the window, Wayland, X11 and Windows keep the button's release from the window, so Slint would keep the title bar's press and start a new move on every later click; `platform::drag_window` therefore reports the pointer as having left the window once the move has begun, which cancels that press on every system. macOS and Linux (Wayland) have been run; Windows compiles by construction and is untested.

**D6 · Dev shell and package from the `chess-flake` workspace** — *current*
The flake composes the workspace's `rustShell` bundle (the name is historical; the bundle is domain-agnostic) and adds cmake for libopus and, on Linux, the libraries the window and the audio output need. The package is built with the workspace's Rust toolchain through `makeRustPlatform`, because nixpkgs' own compiler (1.91) trails what Slint 1.18 requires (1.92). The lock overrides `workspace/nixpkgs` to at least the revision of the NixOS that runs Janis: a development build loads the system's graphics driver from `/run/opengl-driver` at run time, and a Mesa built against a newer glibc than the dev shell's fails to load, which leaves no OpenGL and no window.
Superseded: the `tauriShell` bundle with Node, pnpm and the webkit stack (D6a, first version).

**D47 · `reqwest` for the Jellyfin calls** — *current*
The Jellyfin client uses `reqwest`'s blocking client with rustls: the same version and TLS stack `audio-stack-rs` already links for web radio, so nothing new is built. It is a JSON client, not an audio crate, so it stays outside the facade. The calls run on the server worker, never on the interface thread. Every call goes through one client, built on first use and kept for the process, so its connections stay open between calls: a grid filling with covers asks for picture after picture over the same few connections rather than connecting for each. Measured on a Jellyfin server over the network, the songs list made 22 picture requests over at least 5 connections in its first ten seconds with a client per call, and the same 22 over 2 with the shared one.

## Architecture

**D7 · State modules decide; the controller acts** — *current*
`player`, `equalizer`, `playback_options` and `notifications` hold state and return `Effect`s; `Application`, on the interface thread, carries them out against the engine, the database and the window. Every decision is unit-tested without an audio device or a database, which the stores of the first version could not be.
Superseded: Svelte rune-class stores that called the engine and persisted directly (D7a, first version).

**D8 · Rust and Slint meet only at exported globals** — *current*
`Shell`, `Player`, `Equalizer`, `Library`, `Covers`, `Radio`, `Settings`, `Strings` and `Tokens`. Feature components read their global and call its callbacks; the controller fills and handles them. With seven screens, globals avoid threading dozens of properties through the page, which is what a single-page application like the sibling project super8 can afford to do instead.

**D9 · Selections and filters live in the controller** — *current*
A screen is created anew each time it is opened, so the Library tab and selection, the Radio filters and the queue card's view are controller state. The first version held them in view stores for the same reason (its router destroyed screens on navigation).

**D10 · Nothing slow on the interface thread** — *current*
File reads, picture decoding and library scans run on named worker threads (`background::Worker`); results come back through `application::post`. A station connects on a tokio runtime the controller keeps for as long as it lives: the stream goes on downloading on the runtime that opened it, and a runtime dropped after the connect starves the engine (found when the first station never played). Short writes of one preference are the exception.

**D11 · The player mirror is optimistic, the engine authoritative** — *current* (first version)
A click updates the mirror at once and the engine's next state event confirms or corrects it. Position is interpolated between the engine's ten reports a second. The queue replay the engine offers a host whose mirror can restart is ignored: this mirror lives exactly as long as the engine.

**D12 · Failures reach the listener** — *current*
Playback errors, a station that will not connect, unreadable cover art and a failed library change raise a notification, deduplicated by key where a burst is likely. Failing to save a preference does too, where the first version only logged it.

**D13 · One running instance, found through a loopback port** — *current*
A second launch reads a port number the running instance left in a file beside the database, asks it to come forward, and exits before opening the database: two processes would race the SQLite write lock. A stale file fails to connect and does not block the launch. Standard library only, the same on every platform. The running instance answers the request, and the launch only steps aside on that answer: after a crash the file can name a port another program has since taken. An instance removes the file on exit only while it still holds its own port.
Superseded: Tauri's single-instance plugin (D13a, first version).
Superseded: a launch that stepped aside as soon as it could connect and write to the recorded port (D13b).

**D14 · The data directory keeps its place** — *current*
`janis.db` stays in the platform's per-user data directory under `com.olivierbazin.janis`, the path Tauri resolved, so libraries made by the first version open unchanged.

**D31 · A late result carries the epoch it was asked under** — *current*
A station connection (`Player`'s connection epoch, carried by `Effect::ConnectStation`), a station's announced art and a list thumbnail are each tagged when requested and dropped on arrival if the listener has since chosen something else or the library was replaced — the rule the current track's cover already followed. Without it, the outcome of a station left for another cleared the one being connected, and a slow decode put the previous station's art back.

**D32 · The controller is one file per concern** — *current*
`application.rs` keeps boot, `post`, carrying out effects and shutdown; `application/` holds one file per group of screens (`now_playing_screen`, `library_screens`, `radio_screen`, `settings_screen`, `equalizer_sheet`) and per cross-screen duty (`artwork`, `notices`). What the Now Playing spotlight shows and plays is decided in `library_view`, and the Discover shelves are kept tile by tile like every other list, so a click is an index.
Superseded: one 1,270-line `application.rs` holding every refresh and every wire (D32a).

**D33 · Sliders that are saved once settled are also saved on quit** — *current*
The volume and the equalizer gains wait for the drag to rest before reaching the database; what is still waiting when the application quits is written by `Application::shut_down`.

**D34 · An equalizer band updates its own row** — *current*
The gains model is updated row by row. Replacing it rebuilds every slider — Slint clears a repeater's instances on a model reset — which ends the drag of the one under the pointer.

**D35 · Up Next does not claim an order under shuffle** — *current*
The engine holds the play queue and draws the shuffled order itself, without reporting it. While shuffle is on, the Up Next card still lists what is left to hear, but unnumbered and under a heading that says the queue is shuffled; a click on a row still plays that track. The change stays inside Janis: having the engine report its order, or moving the queue into Janis, were both set aside.

**D36 · Search rebuilds every list, measured** — *current*
A keystroke in the search field rebuilds the Library, Local Files and Radio lists whichever screen is open. On a library of 20,000 tracks that takes about 60 ms in a debug build (the `search refresh` line of `JANIS_LOG=debug`), so tracks are not shared between the lists and the refresh is not narrowed to the open screen. To be looked at again if that figure grows.

**D37 · System dialogs are awaited, a fatal start is shown** — *current*
The file and folder pickers are `rfd`'s asynchronous dialogs, awaited on the interface thread's loop (`slint::spawn_local`), so the window keeps drawing behind them. An error that stops the application from starting is shown in a system message box as well as on standard error, which a launch from the desktop has nowhere to show.
Superseded: blocking pickers called from the click handler; a start error on standard error only (D37a).

**D44 · A Jellyfin server's songs are synced into the library** — *current*
The listener signs in to one Jellyfin server in Settings, and its audio items become rows of `tracks`, keyed by a `jellyfin://<server>/<item>` address and pointing at a `media_servers` row. They then appear wherever a local file does — Library, Discover, search, Now Playing, the queue — with no screen of their own; only Local Files keeps to the files on disk. A sync reads the whole listing (`/Items`, 500 at a time) at launch, after signing in and on demand, upserts it and removes the server's rows it no longer lists; a listing that fails part-way changes nothing, so a server that goes away mid-sync cannot empty the library. Loudness measurements stay outside the upsert, as for files. One server at a time: signing in elsewhere, or signing out, removes the songs with the row. `jellyfin.rs` makes the calls and maps the answers, `media_server.rs` holds the state and decides, and the calls run one after another on the server worker, so a sign-out queued behind a sync lands after it.

**D45 · A server song is the original file, played as a local one** — *current*
The engine fetches `/Audio/{item}/stream?static=true`, the file as it is on the server: no transcoding, so a lossless file stays lossless and the format badge tells the truth. `audio-stack-rs` gained remote queue entries for this (`Location::Remote`): the file downloads into a temporary file as it plays and a seek past what arrived becomes a range request, so seek, gapless, crossfade, normalization and loudness measurement apply unchanged. The next remote track opens on a thread of its own, so preloading never stalls the audio already playing. The token rides in the query, because the engine sends no headers; the engine shows the address without its query in its errors. Covers come from the server's pictures (`/Items/{item}/Images/Primary`, sized to the tile), the album's or else the song's own. Album first because Jellyfin gives a song a picture of its own whenever its file embeds one, nearly always the album's sleeve again: on a library of 905 albums that was 7,187 distinct pictures, each asked for, decoded and cached separately, where album first makes the songs of an album share one (D18). A song whose embedded art truly differs from its album's, as on some compilations, shows the album's. Playing a server song through the radio path was set aside: it has no length, no seek and no preload.
Superseded: the song's own picture first, else its album's (D45a), which asked for the same sleeve once per song.

**D46 · Only the access token is kept** — *current*
The password goes to `/Users/AuthenticateByName` once and is cleared from the form as soon as it is read; `janis.db` keeps the address, the server and user identifiers and the token. The system keychain was set aside: one more platform-dependent dependency, and a failure mode on Linux desktops without a secret service, to protect a token the listener can revoke from the server. A token the server stops accepting asks for the password again and keeps the songs. Janis names itself `Janis` and sends a random device identifier made once (`user_preferences.device_identifier`), nothing personal. Debug forms of the token-carrying types hide it.

**D50 · Every track has a gain before it is first heard** — *current* (2026-11-12)
A track plays at the first of: its album gain, its track gain, its measured loudness against −18 LUFS (the ReplayGain 2.0 reference), else unchanged. The album gain comes first so the songs of one record keep the loudness differences their mastering gave them; shuffling across albums is still evened out. A gain without a peak borrows the other gain's, so a boost stays under full scale wherever a peak is known. Files carry their gains in ReplayGain tags. A Jellyfin server gives `NormalizationGain` and `AlbumNormalizationGain` for each song, already against −18 LUFS, from its own EBU R128 scan or the file's tags, and they fill the same columns as tags; it gives no peak. Local files with no tag are measured on a thread of their own, at the lowest priority (D52) (`audio-stack-rs`'s `measure_file`, the same meter as playback, decoding faster than real time) at start-up and after every scan. A file is tried once per session. A gain measured mid-session applies the next time a queue is loaded, without re-reading the library. Before this, a file was measured only while it played start to finish without a seek, and the gain took effect only at the next launch. On the reference library that left 1 of 233 untagged files measured and none of 13,000 server songs with a gain; the server lists a gain for about 98 % of them. A server song without one is still measured as it plays.
Superseded: track gain only, and measurement only while playing (D50a, first version).

**D52 · Loudness is measured at the lowest priority** — *current* (2026-11-12)
The loudness thread asks the system for the priority it keeps for work no one waits on (`platform::lower_current_thread_priority`): background quality of service on macOS, niceness 19 on Linux, background mode on Windows. At normal priority, measuring held one performance core at 100 % for as long as it ran, about 8 minutes of processor time for 881 files, with the fans to match; playback itself takes 12 %. Measured on 60 files at each level: normal priority (31) took 40 s and a full core; utility (20) took 35 s and still a full core, since it only gives way when something more important asks, so it was set aside; background (4) took about 60 s for 47 files on the internal disk at 15–100 % of an efficiency core. At background priority the system also holds back the thread's reads, and a file on an SMB share took about 40 s instead of half a second. That is accepted: only a few files are measured from a share, and a track is still measured while it plays when it has no gain yet. Because a thread at this priority can wait a long time on a read, it never takes the database lock the engine also waits on: the library thread lists the files to measure and stores each result.
Superseded: the loudness thread at normal priority, writing its results to the database itself (D52a).

## Interface

**D15 · Atomic design in three layers** — *current* (first version)
A domain-agnostic design system (atoms, molecules, organisms), feature folders built on it, and screens that compose features, with one-way dependencies. The component names carried over from the Svelte tree; `Glyph`, `ScreenHeader` and `ScrollArea` are new. The methodology is in `ATOMIC_DESIGN.md`, its application in `.claude/rules/slint-ui.md`.

**D16 · Translated labels are generated properties** — *current*
`build.rs` turns each key of `translations/en.json` into a property of the `Strings` global and generates the Rust function that fills it. A component naming a missing key does not compile, which keeps the check the TypeScript key type gave, and a language change updates every label. Text built in Rust goes through `Translations`. English is the reference; a test holds the two catalogs to the same keys.

**D17 · Long lists draw only what is in view** — *current*
Lists of fixed-height rows are windowed against their screen's `ScrollArea`: the songs list, the Local Files table, the queue card, the accordion, the Radio grid and the Library's album and artist grids, which draw only their lines in view and count the open accordion that pushes the lines under it down. The first version rendered a row per item (up to 1,300 station cards). It is also what keeps the bounded cover cache from chasing its own evictions: an album grid drawn whole over a Jellyfin library of thousands of albums asked for every cover, and the covers in view were evicted as fast as they arrived. A grid card is never narrower than the narrowest card, so the grid has no zero-width moment before its first layout in which hundreds of lines would count as in view.

**D18 · Covers are decoded off-thread into a bounded cache** — *current*
A tile asks for its cover when it appears; the picture is decoded and scaled on a worker, cached (300 thumbnails at most), and a revision counter tells tiles to look again. The first version kept base64 data URLs in the webview under the same limit. The cache is keyed by cover, not by track (`covers::CoverKey`): a server song's key is the item whose picture it shows, so the songs of an album that show the album's picture share one request, one decode and one cache entry. The controller knows each track's key from the library it holds (`Track::artwork_item`), so the interface thread never asks the database. A file keys by its track, its lookup in the file and its album's files being what decides its art.
Superseded: thumbnails cached per track (D18a), which asked a server for the same album picture once per song in a list.

**D19 · No blur: nearly opaque surfaces and a drawn glow** — *current*
Slint has no backdrop blur and no blur filter. The mini player, the notifications and the equalizer scrim are nearly opaque where they were translucent over a blur, and the glow behind the Now Playing art is a picture drawn once from the prism tokens (`halo.rs`) that the interface rotates. The drop zone's outline is solid, there being no dashed stroke either.

**D20 · Everything that moves during playback runs on one 30 fps clock** — *current*
The renderer repaints the whole window for any change, so an animation costs its frame rate. With a live trace at 60 fps the application used 8–13 % of the graphics processor on every screen; on one 30 fps clock for the waveform, the glow and the play button's pulse it uses 5.6–6.7 %, about what the first version did. The glow and the pulse stand still while nothing plays. Caching static regions as textures and removing any single element of the scene made no difference.
Superseded: a 60 fps waveform timer with the glow and the pulse on the display's own frame clock (D20a).

**D21 · An album opens under its card's row** — *current*
The accordion of a selected album or artist sits under the whole row holding its card. The first version's grid placed it immediately after the card, leaving the rest of that row empty.

**D22 · Interface changes are checked on snapshots** — *current*
Debug builds can save what the window shows and be put in a state by environment variables (`JANIS_SNAPSHOT`, `JANIS_SCREEN`, `JANIS_AUTOPLAY`, …). It stands in for a person at the screen for everything but pointer interaction, and replaced the Svelte checker as the routine verification.

**D38 · What is not translated** — *current*
A language is named in its own language ("English", "Français") and the wordmark "JANIS" is a proper noun: both are written in the components that show them. Units and format names ("kHz", "kbps", "FLAC", "Hi-Res") are the same in every language Janis ships in and are formatted in Rust. Everything else comes from the catalogs.

**D39 · A round shape takes its radius from its size** — *current*
A circle or a pill is `self.width / 2` or `self.height / 2`, not a number that has to be kept equal to half the box. The other radii are `Metrics` tokens (`radius-tag`, `radius-segment`, `radius-tile`, `radius-inset` beside the first five), and a tint of a token is a function in `tokens.slint` (`accent-tint`, `play-pulse-at`).

**D42 · Favourite stations: starred on Now Playing, filtered on the Radio screen** — *current*
A station is a favourite or not, by its catalog id, in the `favourite_stations` table (a row is a favourite, no row the default); the id is not checked against the catalog, so a station that leaves it and returns keeps its star. The star that flips it sits after the station name on Now Playing, an `IconButton` stroked when off and filled in the accent when on. The Radio screen shows the favourites two ways: a "Favourites" chip that filters the grid, combining with the country and genre chips and the search, and a filled star after the name on each favourite's card. The card's star is a mark, not a button: the whole card is one click target that plays the station. `favourites.rs` holds the set and decides; `Catalog::filtered` takes the ids as one more filter. `JANIS_FAVOURITES` turns the filter on for snapshots.

**D43 · No Spotify source** — *current*
Janis does not connect to Spotify, and has no screen announcing that it will. Spotify's Web API streams no audio: the most an application can do is remote-control a Spotify Connect device, which needs Premium, with the sound bypassing the engine's equalizer and visualiser. Since 2026 an app in Development Mode serves at most five allow-listed users and its client ID may not be disclosed, so every listener would need a Spotify app of their own. The Rust crates either broke under Spotify's February 2026 changes (`spotify-rs`) or would bring a second HTTP stack. That cost outweighs a source that would only remote-control another application. The placeholder screen, its sidebar entry, its strings, its colour token, its icons and the green variant of `PrimaryButton` went with it.

**D48 · The Jellyfin card and a text field atom** — *current*
The server is set up in a card of the Settings screen, between the audio output and the language: the address, the user name and the password, then a Sign in button; signed in, the server's name, the user and the song count, with Sync now and Sign out. A status line under it says what is happening or what went wrong. Its fields are the `TextField` atom, made for it (a label above an inset input, plain or password). The titlebar search keeps its own inline input. The fields live in the `Jellyfin` global, so a half-typed address survives leaving Settings.

**D49 · The Library tabs are alphabetical** — *current*
Albums by name (then by artist; untitled albums last), artists by name, songs by artist, then album, then title (2026-11-13; a song without an artist or an album after those with one), so the Songs tab reads like a shelf sorted by artist, compared by `text::compare_labels`: case and accents ignored, and whatever leads up to the first letter or digit skipped, so "'Round Midnight" files under R and a stray leading space does not put a title first. Inside an album the running order stays (disc, track, title). The order is worked out once per library (`library_view::LibraryOrder`, positions into the lists the controller already holds), so a search only filters; on a 13,905-song library it takes about 67 ms in a debug build, at launch and after a sync or a rescan. "Recently added", Discover, Local Files and the Now Playing spotlight keep the library's own order, newest addition first, which is what their shelves mean. Leading articles ("The", "Les") are not skipped.
Superseded: albums newest addition first, artists most tracks first, songs newest addition first (D49a), the order Discover still uses.
Superseded: songs by title, then by artist and album (D49b), until 2026-11-13, when the Songs tab was asked to list an artist's songs together.

**D51 · An artist's tracks are listed album by album** — *current* (2026-11-12)
Browsing an artist in the Library lists its tracks grouped by album, albums alphabetical by `text::compare_labels` (untitled last), each album in its running order, and titles each row "Album - Song" ("Electric Ladyland - Voodoo Child") since the album is what tells two takes or two periods apart in a long list. A click plays from that row in this order. Only the panel does this (`library_view::artist_tracks`): the artist grouping keeps the library's order, so the Discover artist shelf and the Now Playing spotlight are unchanged.
Superseded: the artist's tracks in the library's order, titled by song alone (D51a).

## Distribution

**D23 · Installers from cargo-packager, the rpm from cargo-generate-rpm** — *current*
`cargo packager` builds the DMG, the NSIS and MSI installers, the AppImage and the deb, with the artifact names the first version's bundles had, so the macOS install script and the release notes kept working. It has no rpm format; `cargo generate-rpm` builds that from the same binary. No signing certificate exists: macOS is ad-hoc signed, the others unsigned.
Superseded: the Tauri bundler and `tauri-action` (D23a, first version).

**D30 · The dev shell builds cargo-packager on macOS** — *current*
nixpkgs does not carry `cargo-packager`, and `just release-macos` and `install-macos` run inside the dev shell, so `flake.nix` builds it from the published crate at a pinned version, with the lock file the crate ships. Linux and Windows keep `cargo install cargo-packager --locked`: their release recipes run outside Nix, as does the release workflow.
Superseded: a separate `cargo install cargo-packager --locked` on macOS too (D30a), which left the recipe failing in a fresh shell.

**D24 · A locally built macOS bundle is pointed at the system's libraries** — *current*
A binary built in the Nix dev shell links libiconv from the Nix store. A signed bundle with the hardened runtime refuses a library signed by someone else, so the bundle crashed at launch from Finder while the bare binary ran from a terminal. `just release-macos` rewrites the reference to the system's copy before packaging and fails if the bundle still reaches into the store. Release builds from the workflow never had the store path.

**D25 · The disk image embeds no license agreement** — *current*
An embedded agreement makes mounting the image wait for a click. The install script answers the prompt for images of earlier releases, which did embed one.

**D26 · Release recipes per platform, and a local install on macOS** — *current*
`just release-macos`, `release-linux` and `release-windows` are gated to their system; `install-macos` builds, asks a running Janis to quit, and swaps the new bundle into `/Applications` after verifying its signature. The shape comes from the sibling project super8.
Superseded: one `just release` for whichever platform it ran on (D26a).

## Working on the project

**D27 · Instructions for coding agents in `CLAUDE.md` and path-scoped rules** — *current*
`CLAUDE.md` holds the always-loaded essentials (mandatory rules, commands, the module map, the boundary with Slint); rules in `.claude/rules/` load only when matching files are touched (Rust structure, the audio engine, the library and database, the Slint interface with its atomic tiers, packaging, documentation). The arrangement comes from super8.

**D28 · History goes in this log** — *current*
Living documentation describes the present; a decision that is made or changes gets an entry here, and a superseded choice becomes a lettered sub-entry.
Superseded: commit messages as the only decision record (D28a, first version).

**D29 · Atomic commits on the rewrite branch** — *current*
Each step of the rewrite was committed as it passed `just check`, so any step can be returned to.

**D40 · The dev shell carries the Slint language server** — *current*
`slint-lsp` is built in the flake at the version of Slint in `Cargo.lock` (1.18.1): nixpkgs' recipe, with the workspace's Rust toolchain, the matching crate and its own dependency hash. A Slint bump moves its version and both hashes too. What the shell puts on the path is a wrapper that tells the server where the latest build left the generated `strings.slint`, so an editor resolves the `Strings` global; a key added to the catalog is known to it after the next `cargo build`.
Superseded: the release the pinned nixpkgs carries, 1.14.1, which parsed the interface but trailed the compiler by four releases (D40a).

**D41 · The station catalog is built by a Rust package of its own** — *current*
`scripts/radio` is a standalone package (`janis-stations`, own `Cargo.lock`, run with `just stations`) that merges the curated seed with Radio Browser's answers into `data/stations.json`. Standing apart keeps its HTTP client out of the application's dependency graph, so `Cargo.lock` and the flake's `cargoHash` do not move with it. On one captured set of answers it writes the same file, byte for byte, as the script it replaces.
Superseded: `build-stations.mjs`, a Node script, the last thing in the repository that needed Node (D41a).
