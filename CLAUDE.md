# Janis - Open-Source Desktop Audio Player

A native application: Rust throughout, with a [Slint](https://slint.dev) interface, playing through the `audio-stack-rs` engine. See `README.md` for features, `docs/specs.md` for behaviour and `decisions-log.md` for why things are the way they are.

## Mandatory rules

> Break any of these and the task is not done.

1. **`just check` passes.** `cargo fmt --check`, `clippy -D warnings` and the tests are clean before you report a task as done. Run `nix build .#janis` when `flake.nix`, `Cargo.lock` or the build changes.
2. **Separation of concerns follows the module map below.** State modules decide and return `Effect`s; the controller acts; the window shows what it is given. Domain logic never touches Slint, the engine or the database directly. Details: `.claude/rules/rust.md`.
3. **Atomic design in `ui/`.** Tokens → atoms → molecules → organisms → features → screens → the `AppWindow` page. A component only uses tiers below it; Rust and Slint meet only at the exported globals. Reuse before creating; a new variant of a shared component is a decision to ask about. Details: `.claude/rules/slint-ui.md`.
4. **Tokens only.** No colour, typeface, text size, weight, radius, shadow or duration literal outside `ui/tokens.slint`; Rust reads colours from the exported `Tokens` global. One dark "prism" theme: a second one must only need other token values.
5. **The audio path stays behind its boundary.** Decoding, DSP, output, transport, web radio and metadata parsing live in `audio-stack-rs`, reached only through the facade `src/audio.rs` re-exports. Never import an audio or metadata crate directly. Details: `.claude/rules/audio.md`.
6. **Nothing slow on the interface thread, and nothing tied to a screen.** File reads, decoding, scans and connects run on workers and come back through `application::post`. A screen is created anew each time it is opened: selections, filters and running tasks live in the controller.
7. **UI changes are looked at,** not just compiled: run the app with `JANIS_SNAPSHOT` and read the picture. Say so when something could not be checked that way (anything needing a pointer).
8. **Performance stays measured.** The renderer repaints the whole window for any change: everything that moves during playback runs on the controller's one clock, lists draw only the rows in view, and an action refreshes only what it changed. Measure processor and graphics use before and after a rendering change.
9. **No hard-coded UI strings.** Text comes from the `Strings` global or from `Translations`; new keys go in both `translations/en.json` and `fr.json`.
10. **No abbreviations in identifiers** (Rust and Slint). Names imposed by an external interface are the only exception.
11. **Documentation describes the present.** `README.md`, `docs/specs.md`, comments and doc strings say what is, never what used to be. Every user-facing feature is in `docs/specs.md`; every decision that is made or changes gets an entry in `decisions-log.md`. Markdown is never hard-wrapped. Details: `.claude/rules/documentation.md`.
12. **Cross-platform by default.** macOS, Windows and Linux. What differs by system lives in `src/platform.rs` behind a neutral name; platform names stay out of shared interfaces and `.slint` files.
13. **No regressions.** Before completing any task, verify the features of `docs/specs.md` it could have touched.

## Quick reference

```sh
direnv allow                 # or: nix develop — the Rust toolchain, just, cmake
cargo run                    # the app (`just run`, or `run` in the dev shell)
just check                   # fmt check, clippy -D warnings, cargo test
just release-macos           # dist/Janis.app + .dmg (other systems: release-linux, release-windows)
just install-macos           # release-macos, then replace /Applications/Janis.app with the .dmg's
nix run                      # build + launch the release app, no dev shell
JANIS_LOG=debug cargo run    # what the standard-error logger shows
```

Development hooks, debug builds only:

| Variable | Effect |
|---|---|
| `JANIS_SNAPSHOT=<file.png>` | Saves what the window shows after `JANIS_SNAPSHOT_SECONDS` (2) and quits |
| `JANIS_SCREEN=<identifier>` | Opens on that screen: `now-playing`, `library`, `discover`, `radio`, `local`, `spotify`, `settings` |
| `JANIS_LIBRARY=<tab>[:<card>]` | Opens a Library tab and, in a grid, the card at that place |
| `JANIS_SEARCH=<text>` | Fills the search field |
| `JANIS_EQUALIZER` | Opens the equalizer sheet |
| `JANIS_AUTOPLAY=<search>` | Plays, silently, the tracks the library search finds |
| `JANIS_AUDIBLE` | With `JANIS_AUTOPLAY`: keeps the saved volume, so the trace is live |
| `JANIS_STATION=<place>` | Starts the station at that place in the Radio grid |

## Module map

| Module | Job | May depend on |
|---|---|---|
| `src/main.rs` | Composition: logger, the one-instance claim, backend, window, controller; the development hooks | everything |
| `src/application.rs`, `src/application/*.rs` | The controller on the interface thread: boot, carrying out effects, engine events, covers, notifications, refreshes; one file per group of screens (`now_playing_screen`, `library_screens`, `radio_screen`, `settings_screen`, `equalizer_sheet`) and per shared duty (`artwork`, `notices`) | every module, generated UI types |
| `src/effects.rs` | `Effect`: what a state module asks the controller to do | `library`, `persistence`, `radio` (types only) |
| `src/player.rs` | The playback mirror: what plays, transport flags, position interpolation | `audio` (event types), `effects` |
| `src/equalizer.rs`, `src/playback_options.rs` | The EQ (bands, presets, linear phase); the four playback switches | `audio` (constants), `effects`, `persistence` (types) |
| `src/notifications.rs` | Error notifications, deduplicated by key | nothing |
| `src/library_view.rs` | Pure: album and artist grouping, search, the Now Playing lists, the Library selection | `library` (types), `text`, `translations` |
| `src/radio.rs` | The station catalog (`data/stations.json`) and its filters | `audio` (types), `text`, `translations` |
| `src/visualizer.rs`, `src/halo.rs` | The oscilloscope trace as path commands; the glow picture behind the Now Playing art | nothing (`halo`: `covers` type) |
| `src/covers.rs` | Cover art from base64 to pixels; the bounded thumbnail cache | `library`, `persistence` |
| `src/translations.rs`, `src/text.rs` | The catalogs and lookups; label ordering and formatting | `library` (types) |
| `src/library.rs`, `src/persistence.rs` | The scanner, watched folders, cover lookup; the database, its migrations, the preferences. The only SQL | `audio` (facade functions), rusqlite, walkdir |
| `src/audio.rs` | The `audio-stack-rs` facade re-exports and the `EngineBridge` event sink | `audio_stack_rs` — the only module that names it |
| `src/background.rs`, `src/instance.rs`, `src/logging.rs`, `src/platform.rs` | Worker threads; the one-instance guard; the logger; what differs by operating system | std, `log`, Slint's winit window |
| `ui/` | The Slint interface in atomic tiers (see `.claude/rules/slint-ui.md`) | tokens and lower tiers |

## Data flow

```
a screen          → callback on a Slint global (Library.song-clicked, Player.toggled, …)
Application       → state module method → Vec<Effect> → Application::run
                    → AudioEngine (load_queue, toggle, set_eq, …)  /  persistence  /  workers
engine thread     → decode → resample → ring → output callback (EQ, gain, analyser tap)
engine            → EngineBridge → post() → Player::apply → effects → refresh_player
                  → latest visual frame, read by the 30 Hz frame timer → Visualizer → path commands
a station click   → Player::play_station → Effect::ConnectStation → the station runtime → same path
```

| Slint global | File | Carries |
|---|---|---|
| `Shell` | `ui/shell.slint` | Open screen, search text, equalizer sheet, notifications, window drag |
| `Player` | `ui/features/player/player.slint` | What is playing, transport, waveform, animation clock, queue card, artist spotlight |
| `Equalizer` | `ui/features/equalizer/equalizer.slint` | Gains, presets, linear phase |
| `Library` | `ui/features/library/library.slint` | Library, Local Files and Discover lists; library actions |
| `Covers` | `ui/features/library/covers.slint` | Thumbnails by track id, loaded as tiles appear |
| `Radio` | `ui/features/radio/radio.slint` | Filter chips, stations, the loaded station |
| `Settings` | `ui/features/settings/settings.slint` | Playback switches, output device, language |
| `Strings` | generated by `build.rs` | Every translated label |
| `Tokens` | `ui/tokens.slint` | Design tokens (read by Rust for the glow's colours) |

## Stack and distribution

- **Interface:** Slint 1.18 (winit backend, FemtoVG renderer), compiled from `ui/app.slint` by `build.rs`, fully custom components.
- **Audio:** `audio-stack-rs` (MIT, a git dependency pinned to a tag; `Cargo.lock` records the commit). Janis injects `EventSink` and `Store` and drives the `AudioEngine` facade.
- **Persistence:** SQLite (`rusqlite`, bundled): `janis.db` in the per-user data directory holds preferences and the track library.
- **Releases:** pushing a `vX.Y.Z` tag runs `.github/workflows/release.yml`, which builds every platform's installers with `cargo packager` (and the rpm with `cargo generate-rpm`) and attaches them to a draft release; the tag must match `Cargo.toml` and `flake.nix`. No signing certificate exists: macOS is ad-hoc signed, the others unsigned. Nix users install from the flake. Details: `.claude/rules/packaging.md`.

## Tools

| Tool | When |
|---|---|
| `mcp__context7__resolve-library-id` + `query-docs` | Slint (language and Rust API), winit, and the audio crates: use live docs, not memory |
| `.cargo/registry/src/*/<crate>-<version>/` | Read the exact source of a dependency (Slint, winit) instead of guessing its interface. The registry lives in the project |
| `ast-grep run --lang rust -p '<pattern>'`, LSP | Structural search and refactoring in Rust; `grep` for `.slint` files and plain text |
| `git log`, `decisions-log.md` | Before reversing a choice, read why it was made |
