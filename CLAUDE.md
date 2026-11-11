# Janis - Open-Source Desktop Audio Player

A native application: Rust throughout, with a Slint interface.

## Mandatory Rules

> Break any of these and the task is not done.

1. **Tokens only.** Components never write a color, typeface, text size, font weight, radius, shadow or duration literal: every one comes from `ui/tokens.slint` (`Tokens`, `Fonts`, `Typography`, `Metrics`, `Motion`). Rust reads colors from the exported `Tokens` global, never from literals. Before adding a value, read `tokens.slint` and pick the closest existing token; if none fits, add one with a semantic name. Layout lengths — padding, spacing, the size of a fixed box — may be written where they are used.
2. **Single theme, token-routed.** Janis ships one dark "prism" theme. A second theme must only require other values in `tokens.slint`, so no component may hard-couple to a specific color.
3. **Look at every interface change.** Compiling is not seeing. After editing a `.slint` file, run the app with `JANIS_SNAPSHOT` (see Development hooks) and look at the picture.
4. **No regressions.** Before completing any task, verify every feature in `docs/specs.md` still works.
5. **Keep `docs/specs.md` current.** Every new user-facing feature must be described there before the task is done.
6. **`IconButtonIdentifier` registry.** Adding any new icon-only button requires a corresponding entry in the `IconButtonIdentifier` global (`ui/atoms/icon_button.slint`); the button carries it as its accessible id.
7. **No hard-coded UI strings.** Text in `.slint` comes from the `Strings` global (generated from `translations/en.json`, one property per key — a missing key does not compile); text built in Rust comes from `Translations`. New keys go in both `translations/en.json` and `fr.json`; a test checks they hold the same keys.
8. **Screens are thin containers.** A screen lays out feature components and tells lists what is in view; it does not own detailed markup. When a group grows beyond a few lines, extract it into a named component at the correct layer (design system if domain-agnostic, the feature folder otherwise).
9. **Long-running tasks, and any selection or filter a screen offers, are independent of screens.** A screen is created anew each time it is opened, so anything the listener should find again — an album browsed in Library, a genre filter in Radio, the queue card's view — lives in the application controller, never in a property of the screen. Playback, scans and connects are never tied to what is on screen.
10. **State modules decide; the controller acts.** The modules that hold state (`player`, `equalizer`, `playback_options`, `notifications`) never touch the engine, the database or a timer: their methods update the state and return `Effect`s, and `Application` carries them out. That is what keeps every decision unit-tested without an audio device. Do not call the engine or the database from a state module, and do not put decisions in the controller.
11. **Nothing slow on the interface thread.** File reads, picture decoding and scans run on a `background::Worker`; network waits run on the station runtime; results come back through `application::post`. Short SQLite writes of a preference are the one exception.
12. **Refresh what changed.** Window callbacks push only the playback scalars (`refresh_player`). An action refreshes the lists it changed itself; `refresh_everything` is for a new library or another language. A volume drag must never rebuild a list.
13. **Long lists draw only what is in view.** A list of fixed-height rows is windowed against its screen's `ScrollArea` (see `TrackList`, `LocalTable`, the Radio grid). Never instantiate a row per item of a list that can grow with the library.
14. **Reuse-first.** Before writing new markup, check `ui/atoms`, `ui/molecules`, `ui/organisms` **first**, then the active feature's own folder. A pattern repeated 3+ times inline is a missing atom — extract it. Inline re-implementation of an existing component is a bug.
15. **New UI follows a search → ask → create decision tree.** Search the design system, then the feature folder. Exact match → use it. Close match needing a new variant or property → STOP and ASK before extending (widening a design-system component's interface changes the contract for every consumer). No match → create at the correct tier AND the correct layer. Methodology in [`ATOMIC_DESIGN.md`](ATOMIC_DESIGN.md).
16. **AST / LSP over string-pattern search.** For finding or refactoring Rust structure use `ast-grep` (`ast-grep run --lang rust -p '<pattern>'`); for resolving symbols use the LSP tool. Reach for `grep`/`rg` only for genuinely non-structural text, and for `.slint` files.
17. **Domain logic lives behind the right boundary.** The whole audio path — decoding, the EQ, the analyser, output-device management, the transport, web radio, **and audio-file metadata parsing** — lives in the standalone **`audio-stack-rs`** crate (an MIT project of its own, consumed as a git dependency pinned to a tag), reached only through its facade, which `src/audio.rs` re-exports. Never import an audio or metadata crate (cpal, symphonia, lofty, …) directly, and never name `audio_stack_rs` outside `audio.rs`. Library scanning, persistence and filesystem walking stay in Janis (`library.rs`, `persistence.rs`).
18. **Every mechanism is cross-platform.** Janis targets macOS, Windows and Linux. What differs by system lives in `src/platform.rs` behind a neutral name; platform names stay out of shared docs, interfaces and `.slint` files.
19. **Documentation describes the present, concisely.** What it is, why it exists, how it is used — nothing else. Never narrate the past in living documentation; that's the decision log's job (commit messages).

## Tools to Use

| Tool                                              | When                                                                                     |
| ------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `mcp__context7__resolve-library-id` + `query-docs` | Slint (language and Rust API), winit, and the audio crates — use live docs, not memory   |
| `ast-grep` / LSP                                  | Structural code search & refactor (rule 16)                                              |
| `JANIS_SNAPSHOT` and the other development hooks  | After every interface change (rule 3)                                                    |

## Quick Reference

```bash
cargo run              # the app (`just run`, or `run` in the dev shell). Run inside the nix/direnv shell.
just check             # every gate: rustfmt check, clippy -D warnings, tests
just release           # installers of this platform into dist/ (needs `cargo install cargo-packager`)
nix run                # build + launch the release app — no dev shell
```

The dev shell comes from `flake.nix` (composed from the `chess-flake` workspace's `rustShell` bundle — the Rust toolchain and its tools; the name is historical, the bundle is domain-agnostic — plus cmake for libopus and, on Linux, the libraries the window and the audio output need). `direnv allow` once, or prefix commands with `nix develop --command`.

### Development hooks (debug builds only)

| Variable                         | Effect                                                                    |
| -------------------------------- | ------------------------------------------------------------------------- |
| `JANIS_SNAPSHOT=<file.png>`      | Saves what the window shows after `JANIS_SNAPSHOT_SECONDS` (2) and quits  |
| `JANIS_SCREEN=<identifier>`      | Opens on that screen (`now-playing`, `library`, `discover`, `radio`, `local`, `spotify`, `settings`) |
| `JANIS_LIBRARY=<tab>[:<card>]`   | Opens a Library tab and, in a grid, the card at that place                |
| `JANIS_SEARCH=<text>`            | Fills the search field                                                    |
| `JANIS_EQUALIZER`                | Opens the equalizer sheet                                                 |
| `JANIS_AUTOPLAY=<search>`        | Plays, silently, the tracks the library search finds                      |
| `JANIS_STATION=<place>`          | Starts the station at that place in the Radio grid                        |
| `JANIS_LOG=<level>`              | What the standard-error logger shows (all builds)                         |

## Quality gates — local; CI only builds releases

`just check` runs them all: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. Run it before completing any task.

## Distribution

Bundles come from `.github/workflows/release.yml`: pushing a `vX.Y.Z` tag builds macOS (Apple Silicon + Intel DMGs), Windows (NSIS + MSI) and Linux (AppImage, deb, rpm; x86_64 + arm64) and attaches them to a draft release. The workflow refuses a tag whose version differs from `Cargo.toml` or `flake.nix` — bump both together. The installers are built by `cargo packager`, configured in `Cargo.toml` (`[package.metadata.packager]`); the rpm, which the packager cannot make, by `cargo generate-rpm`. No signing certificate exists on any platform: macOS is ad-hoc signed (`signing-identity = "-"`), Windows and Linux are unsigned; README's Install section documents the first-launch step each platform asks for. `scripts/macos/install.sh` (run through `curl | sh`) is the prompt-free macOS route: it resolves the latest tag from the releases redirect, downloads the DMG for the machine's architecture, copies the app into Applications, verifies the signature and clears the quarantine flag. Nix users install from the flake itself (`nix profile install github:obazin/janis`, or `overlays.default` in a system configuration); the package is built with the workspace's Rust toolchain, and on macOS it installs `Applications/Janis.app` with `bin/Janis` pointing into it. Linux packages declare ALSA explicitly.

## Stack

- **Interface:** Slint 1.18 (winit backend, FemtoVG renderer), compiled from `ui/app.slint` by `build.rs`. Fully custom components; no standard widget style.
- **Application:** one Rust crate. Direct dependencies besides Slint: `audio-stack-rs`, `rusqlite`, `walkdir`, `image` (cover decoding), `base64`, `rfd` (file dialogs), `tokio` (the station runtime), `dirs`, `serde`/`serde_json`, `log`.
- **Audio:** the `audio-stack-rs` crate (MIT, a git dependency pinned to a tag; `Cargo.lock` records the exact commit) — a self-contained engine using `cpal` (output), `symphonia` (decode, plus a libopus-backed Opus decoder of its own), `rubato` (resampling), `biquad` (10 peaking filters, with a linear-phase FIR alternative behind the crate's `fir-eq` feature), `realfft` (analyser), `rtrb` (lock-free ring), `stream-download` + `icy-metadata` (web radio, plus station now-playing APIs), `ebur128` (loudness), and `lofty` (tags/properties/cover art). Janis injects two traits — `EventSink` (outbound events + frames) and `Store` (loudness persistence) — and drives the `AudioEngine` facade.
- **Persistence:** SQLite (`rusqlite`, bundled) — `janis.db` in the per-user data directory holds preferences + the track library.

## Architecture

### Modules (`src/`)

| Module                                   | Holds                                                                                                              |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `main.rs`                                | Entry point: logger, single instance, backend, window, controller; the development hooks                           |
| `application.rs`, `application/*.rs`     | The controller on the interface thread: boot, effects, engine events, covers, notifications, refreshes — and one file per group of screens (`library_screens`, `radio_screen`, `settings_screen`) |
| `effects.rs`                             | `Effect`: what a state module asks the controller to do                                                            |
| `player.rs`                              | The playback mirror: what plays, transport flags, position interpolation                                           |
| `equalizer.rs`, `playback_options.rs`    | The EQ (bands, presets, linear phase) and the four playback switches                                               |
| `notifications.rs`                       | Error notifications, deduplicated by key                                                                           |
| `library_view.rs`                        | Pure functions over tracks: album and artist grouping, search, the Now Playing lists, the Library selection        |
| `radio.rs`                               | The station catalog (`data/stations.json`) and its filters                                                         |
| `visualizer.rs`, `halo.rs`               | The oscilloscope trace as path commands; the glow picture behind the Now Playing art                               |
| `covers.rs`                              | Cover art from base64 to pixels, and the bounded thumbnail cache                                                   |
| `translations.rs`, `text.rs`             | The catalogs and lookups; label ordering and formatting                                                            |
| `library.rs`, `persistence.rs`           | The scanner, watched folders and cover lookup; the database, its migrations and the preferences                    |
| `audio.rs`                               | The `audio-stack-rs` facade re-exports and the `EngineBridge` event sink                                           |
| `background.rs`, `instance.rs`, `logging.rs`, `platform.rs` | Worker threads; the one-instance guard; the logger; what differs by operating system            |

### Playback data flow

```
a screen          → callback on a Slint global (Library.song-clicked, Player.toggled, …)
Application       → state module method → Vec<Effect> → Application::run
                    → AudioEngine (load_queue, toggle, set_eq, …)  /  persistence  /  workers
engine thread     → symphonia decode → rubato → SPSC ring
cpal callback     → 10×biquad (EQ) → volume+track gain → analyser tap → device
                    (the only realtime context: no alloc, no lock, no log)
engine            → EngineBridge → post() → Player::apply → effects → refresh_player
                  → latest visual frame, read by the 60 Hz frame timer → Visualizer → path commands
a station click   → Player::play_station → Effect::ConnectStation → the station runtime
                    (HTTP connect + prefetch off the interface thread) → same decode/EQ/analyser path
```

### Rust ↔ Slint boundary

The screens read exported globals and call their callbacks; `Application` fills the globals and handles the callbacks. Nothing else crosses.

| Global      | File                                    | Carries                                                              |
| ----------- | --------------------------------------- | -------------------------------------------------------------------- |
| `Shell`     | `ui/shell.slint`                        | Open screen, search text, equalizer sheet, notifications, window drag |
| `Player`    | `ui/features/player/player.slint`       | What is playing, transport, waveform, queue card, artist spotlight   |
| `Equalizer` | `ui/features/equalizer/equalizer.slint` | Gains, presets, linear phase                                         |
| `Library`   | `ui/features/library/library.slint`     | Library, Local Files and Discover lists; library actions             |
| `Covers`    | `ui/features/library/covers.slint`      | Thumbnails by track id, loaded as tiles appear                       |
| `Radio`     | `ui/features/radio/radio.slint`         | Filter chips, stations, the loaded station                           |
| `Settings`  | `ui/features/settings/settings.slint`   | Playback switches, output device, language                           |
| `Strings`   | generated by `build.rs`                 | Every translated label                                               |
| `Tokens`    | `ui/tokens.slint`                       | Design tokens (read by Rust for the glow's colors)                   |

Which screen is open is interface state (`Shell.screen`, a string identifier); the sidebar sets it directly.

## Slint notes

- Element-level `opacity` on an animated item forces an offscreen layer (hundreds of megabytes); carry transparency in the color.
- A stroked `Path` is laid out inside its stroke: grow it by the stroke width to keep it on its grid (see `Glyph`, `Waveform`).
- A `Path` with a viewbox keeps its aspect ratio; give it a viewbox equal to its own size to fill a box.
- There is no blur and no dashed stroke. Soft edges are drawn into a picture (`halo.rs`); surfaces that were translucent-over-blur are nearly opaque.
- `FlexboxLayout` wraps (chips, presets); the other layouts do not.
- Some names cannot be property names (`color`, `row`, and an enum value's name shadowed by a property, e.g. `cover`).

## Design System

Three layers with one-way dependencies between them (full methodology in [`ATOMIC_DESIGN.md`](ATOMIC_DESIGN.md)):

- Design system (`ui/atoms`, `ui/molecules`, `ui/organisms`) → tokens and icons only — atoms `Glyph`, `IconButton`, `Chip`, `Badge`, `PrimaryButton`, `GhostButton`, `SectionLabel`, `Eyebrow`, `Toggle`, `ArtTile`, `EmptyState`; molecules `NavItem`, `ToggleRow`, `PlayCircle`, `ScreenHeader`, `ScrollArea`; organisms `SidebarNav`, `Toaster`.
- `ui/features/<x>/` → design system first. Allowed cross-feature edges: the pair `library ↔ player` (the artist spotlight plays through the player; the queue card shows library covers) and `player → equalizer` (the EQ buttons sit in the player's components). Feature components may read `Shell`.
- `ui/screens/` → any feature + design system. Screens are the only layer that freely crosses feature boundaries.

A design-system file importing from `features/`, or a feature-to-feature edge not on the list above, is a design smell — surface it.

Glyphs live as path data in `ui/icons.slint` and render through the `Glyph` atom — no path data in a component.
