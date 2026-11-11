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
macOS, Windows and Linux. `src/platform.rs` holds what differs behind neutral names: the data directory, the title bar the application draws itself where the system keeps its window buttons over it, window dragging, bringing the window forward, files dropped on the window. Only macOS has been run; the other two compile by construction and are untested.

**D6 · Dev shell and package from the `chess-flake` workspace** — *current*
The flake composes the workspace's `rustShell` bundle (the name is historical; the bundle is domain-agnostic) and adds cmake for libopus and, on Linux, the libraries the window and the audio output need. The package is built with the workspace's Rust toolchain through `makeRustPlatform`, because nixpkgs' own compiler (1.91) trails what Slint 1.18 requires (1.92).
Superseded: the `tauriShell` bundle with Node, pnpm and the webkit stack (D6a, first version).

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

## Interface

**D15 · Atomic design in three layers** — *current* (first version)
A domain-agnostic design system (atoms, molecules, organisms), feature folders built on it, and screens that compose features, with one-way dependencies. The component names carried over from the Svelte tree; `Glyph`, `ScreenHeader` and `ScrollArea` are new. The methodology is in `ATOMIC_DESIGN.md`, its application in `.claude/rules/slint-ui.md`.

**D16 · Translated labels are generated properties** — *current*
`build.rs` turns each key of `translations/en.json` into a property of the `Strings` global and generates the Rust function that fills it. A component naming a missing key does not compile, which keeps the check the TypeScript key type gave, and a language change updates every label. Text built in Rust goes through `Translations`. English is the reference; a test holds the two catalogs to the same keys.

**D17 · Long lists draw only what is in view** — *current*
Lists of fixed-height rows are windowed against their screen's `ScrollArea`: the songs list, the Local Files table, the queue card, the accordion and the Radio grid. The first version rendered a row per item (up to 1,300 station cards). It is also what keeps the bounded cover cache from chasing its own evictions.

**D18 · Covers are decoded off-thread into a bounded cache** — *current*
A tile asks for its cover when it appears; the picture is decoded and scaled on a worker, cached (300 thumbnails at most), and a revision counter tells tiles to look again. The first version kept base64 data URLs in the webview under the same limit.

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
