# Janis - Open-Source Desktop Audio Player

## Mandatory Rules

> Break any of these and the task is not done.

1. **Svelte MCP after every `.svelte` edit.** Call `mcp__svelte__svelte-autofixer` on the changed file. Fix all reported issues. Repeat until clean.
2. **Svelte 5 runes only.** Never use Svelte 4 syntax (`$:`, `export let`, `on:event`, `<slot>`, `createEventDispatcher`).
3. **Semantic tokens only.** Components never reference palette colors (`pink-*`, `purple-*`), hex literals, or raw OKLCH values. Use `bg-canvas`, `text-text`, `bg-panel`, `text-accent`, `text-text-muted`, etc. Canvas-drawing code reads the same tokens via `visualPalette()` (`features/player/visualizer.ts`) — never a color literal in a component or draw call.
   3a. **No Tailwind magic numbers.** Never write arbitrary-value classes (`text-[13px]`, `gap-[10px]`, `duration-[120ms]`, etc.). Every size, spacing, duration, and radius must use a token defined in `src/app.css` `@theme` or Tailwind's dynamic 4px spacing scale (`px-3.75`, `h-47.5` are fine — they are scale values, not brackets). Grid templates that can't be expressed on the scale (`grid-cols-[repeat(auto-fill,minmax(180px,1fr))]`) are the one tolerated bracket form. Before writing any new size value, read `app.css` first and pick the closest existing token. If no token fits, add one to `@theme` with a semantic name.
4. **Single theme, token-routed.** Janis ships one dark "prism" theme. All colors flow through the `@theme` tokens in `src/app.css`; adding a second theme later must only require a `[data-theme]` override block, so no component may hard-couple to a specific color value.
5. **Slash-opacity syntax only.** Use `bg-accent/12`, never `bg-opacity-*`.
6. **OKLCH for new colors.** Never add hex or HSL to `app.css`.
7. **No regressions.** Before completing any task, verify every feature in `docs/specs.md` still works.
8. **Keep `docs/specs.md` current.** Every new user-facing feature must be described there before the task is done.
9. **`IconButtonIdentifier` enum.** Adding any new icon-only button requires a corresponding entry in `src/lib/design-system/atoms/IconButtonIdentifier.ts`.
10. **No hard-coded UI strings.** All user-facing text must use `t('key')` from `$lib/i18n/LanguageStore.svelte`. New keys must be added to both JSON translation files (`en.json`, `fr.json`) — the `TranslationKey` type is derived automatically from `en.json`.
11. **`*Screen.svelte` files are thin containers.** Screen components wire data, handle events, and assemble child components — they do not own detailed markup. When a logical UI group grows beyond a few lines, extract it into a dedicated named component at the correct layer (design-system if domain-agnostic, the feature folder otherwise).
12. **Long-running task lifecycles, and any selection or filter a screen offers, are independent of UI lifecycles.** Playback, scans and long IPC calls live in stores, not in screen components — never tear down a task in `onDestroy` or any UI-bound hook; screens subscribe to task state, they do not own it. (The audio keeps playing whatever screen is open; the mini player is proof.) **Selections are persisted**: the router destroys and recreates a screen component on every navigation, so a plain `let x = $state(...)` declared inside one is lost the moment you leave it. Anything the user should still see when they come back — an album browsed in Library, a genre filter in Radio, a view toggle in Now Playing — belongs in a rune-class store with public `$state` fields (see the state-management table below) even when it has no persistence or IPC of its own, never in screen-local state.
13. **Store shape follows persistence.** Stores that persist or have side effects (IPC, the audio engine) expose **setter methods** as the single chokepoint for writes — `playerStore.setVolume(v)`, `eqStore.setBand(i, v)`. Pure ephemeral state exposes **public `$state` fields**. Both shapes are rune classes in `.svelte.ts` files. Never expose a public `$state` field on a store that has persistence or side effects.
14. **Persistent stores boot from `+layout.svelte`'s `bootPromise` and gate UI render on it.** Persistence is SQLite-backed (`janis.db`) via Tauri IPC, so store hydration is async. `+layout.svelte` builds a `bootPromise` (gated by `browser` so prerender stays a no-op) that fetches preferences + the library, then `{#await bootPromise then}` wraps the entire layout. Don't sprinkle `init()` calls across screens; don't put init in `onMount`.
15. **Reuse-first.** Before writing new markup, check `src/lib/design-system/{atoms,molecules,organisms}/` **first**, then the active feature's own folder. Use existing components instead of duplicating their styling inline. A pattern repeated 3+ times in inline form is a missing atom — extract it. Inline re-implementation of an existing component is a bug.
16. **New UI follows a search → ask → create decision tree.** Search the design system, then the feature folder. Exact match → use it. Close match needing a new variant/prop → STOP and ASK before extending (widening a design-system component's API changes the contract for every consumer). No match → create at the correct tier AND the correct layer (design-system only if genuinely domain-agnostic; otherwise the feature folder). Methodology in [`ATOMIC_DESIGN.md`](ATOMIC_DESIGN.md).
17. **AST / LSP over string-pattern search.** For finding/refactoring code structure use `ast-grep` (`ast-grep run --lang ts -p '<pattern>'`, also `rust`); for resolving symbols use the LSP tool. Reach for `grep`/`rg` only for genuinely non-structural text. `.svelte` files are NOT parsed by the built-in `ts` grammar — use Grep/LSP for logic inside `.svelte` `<script>` blocks.
18. **Domain logic lives behind the right boundary.** The whole audio path — decoding, the EQ, the analyser, output-device management, the transport, web radio, **and audio-file metadata parsing** — lives in the standalone **`audio-stack-rs`** crate (a sibling MIT project at `../audio-stack-rs`, consumed as a `path` dependency), reached only through its facade. `src-tauri/src/audio/` is now just the Tauri bridge: the thin `#[tauri::command]` wrappers (`commands.rs`) and the `EventSink` that forwards engine output onto the webview channels (`mod.rs`). Library scanning, persistence and filesystem walking stay in Janis (`library.rs`, `persistence.rs`), calling the facade's `read_metadata`/`read_cover` for per-file parsing. Never import an audio/metadata crate (cpal, symphonia, lofty, …) into `src-tauri` directly — go through `audio-stack-rs`. Commands return pure data (wire types in `src/lib/models/`); PCM never crosses IPC, only 170-byte visualiser frames and transport events do. The frontend renders data and sends commands; it does not re-derive what Rust already computed.
19. **Every mechanism is cross-platform.** Janis targets macOS, Windows and Linux. Data directories come from Tauri's path API, dialogs from the dialog plugin; the webview never touches the filesystem (audio decodes in Rust, cover art crosses IPC as base64 — the asset protocol is deliberately not enabled). Never adopt a platform-specific mechanism as _the_ design; platform names stay out of shared docs and APIs (the macOS-only `titleBarStyle: Overlay` is config, ignored elsewhere — the titlebar component adapts by platform).
20. **Documentation describes the present, concisely.** What it is, why it exists, how it is used — nothing else. Never narrate the past in living documentation; that's the decision log's job (commit messages).

## Tools to Use

| Tool                                                                    | When                                                                                                      |
| ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `mcp__svelte__svelte-autofixer`                                         | After every `.svelte` edit (mandatory)                                                                    |
| `mcp__svelte__get-documentation`                                        | Before writing Svelte 5 / SvelteKit 2 code                                                                |
| `mcp__context7__resolve-library-id` + `mcp__context7__get-library-docs` | Tauri v2 plugin APIs, and the audio crates (cpal, symphonia, rubato) — use live docs, not training memory |
| `ast-grep` / LSP                                                        | Structural code search & refactor (rule 17)                                                               |

## Quick Reference

```bash
pnpm tauri dev         # Full Tauri app with hot reload (preferred). Run inside the nix/direnv shell.
pnpm dev               # Vite dev server only (port 1420) — IPC calls fail outside Tauri
pnpm tauri build       # Release bundle
pnpm build             # SvelteKit static build to /build
pnpm check             # svelte-check + TypeScript validation
pnpm test:unit         # vitest
just fmt-rust          # format the Rust sources; `fmt-rust-check` to verify only
just clippy            # cargo clippy -D warnings
```

The dev shell comes from `flake.nix` (composed from the `chess-flake` workspace's `tauriShell` bundle — Rust toolchain, Node 22, pnpm, prettier; the name is historical, the bundle is domain-agnostic). `direnv allow` once, or prefix commands with `nix develop --command`.

## Quality gates — local, no CI yet

Quality gates are the local suites: `pnpm check`, `pnpm test:unit`, `cargo test`, `cargo clippy`, `just fmt-rust-check`. Run them before completing any task.

## Stack

- **Frontend:** Svelte 5 (runes) + SvelteKit 2 + Tailwind v4 + TypeScript
- **Backend:** Rust via Tauri v2 IPC commands (`tauri-plugin-single-instance`, `tauri-plugin-log`, `tauri-plugin-dialog`). Janis's `src-tauri` depends only on Tauri, `rusqlite`, `walkdir`, and the `audio-stack-rs` facade — no audio/metadata crate is a direct dependency.
- **Audio:** the `audio-stack-rs` crate (`../audio-stack-rs`, MIT, `path` dependency) — a self-contained engine using `cpal` (output), `symphonia` (decode, plus a libopus-backed Opus decoder of its own), `rubato` (resampling), `biquad` (10 peaking filters), `realfft` (analyser), `rtrb` (lock-free ring), `stream-download` + `icy-metadata` (web radio, plus station now-playing APIs), `ebur128` (loudness), and `lofty` (tags/properties/cover art). Janis injects two traits — `EventSink` (outbound events + frames) and `Store` (loudness persistence) — and drives the `AudioEngine` facade. There is no `<audio>` element: every source decodes in Rust
- **Persistence:** SQLite (`rusqlite`, bundled) — `janis.db` in the app-data dir holds preferences + the track library
- **Build:** Vite, adapter-static (SSG — no Node server at runtime)

## Architecture

### Frontend-Backend Boundary

No Node.js server. All backend logic runs in Rust, called via Tauri IPC:

```typescript
import { invoke } from '@tauri-apps/api/core';
const result = await invoke<ReturnType>('command_name', { param: value });
```

### Tauri IPC Commands (registered in `src-tauri/src/main.rs`)

`main.rs` is entry point + handler registration only; commands live in feature modules.

| Command                                                    | Purpose                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Preference commands (6, in `src-tauri/src/persistence.rs`) | `get_preferences` — the single boot fetch (volume, EQ gains+preset, playback switches, title-bar frame switch, language). `set_volume(volume)` / `set_eq(gains, preset)` / `set_playback_option(option, enabled)` (closed enum: `gapless` \| `crossfade` \| `normalize` \| `exclusive`) / `set_title_bar_hidden(hidden)` / `set_language(language)` (`en` \| `fr`). |
| Audio commands (20, in `src-tauri/src/audio/commands.rs`)  | `audio_subscribe(events, frames)` — attaches the two push channels and replays state (including the queue's track ids). Transport: `audio_load_queue(tracks, index)`, `audio_play_stream(stationId, url, nowPlaying)` (the one async command — it awaits the HTTP connect and prefetch so the engine thread never blocks on the network; `nowPlaying` names the station's metadata provider, or null), `audio_play` / `audio_pause` / `audio_toggle` / `audio_stop`, `audio_next` / `audio_previous` / `audio_jump_to(index)`, `audio_seek(positionSecs)`. Parameters: `audio_set_volume(volume)` / `audio_set_eq(gains)` (straight to the realtime atomics, so a slider is audible on the next buffer), `audio_set_shuffle` / `audio_set_repeat` / `audio_set_normalize(enabled)` / `audio_set_gapless(enabled)` / `audio_set_crossfade(enabled)` (the latter two choose how the engine joins one track into the next — see `docs/specs.md`). Output: `audio_devices` / `audio_set_device(deviceId)`. |
| Library commands (7, in `src-tauri/src/library.rs`)        | `list_tracks` — the whole library, newest first. `list_watched_folders` — folders + per-folder counts. `add_watched_folder(path)` — register + scan (blocking pool; the walk runs without the DB lock), returns a `ScanReport`. `import_files(paths)` — ad-hoc single files (dialog / OS drag-and-drop), `folder_id NULL`. `remove_watched_folder(folderId)` — cascade-deletes its tracks. `rescan_library` — re-walk all folders + prune vanished files (only under still-reachable roots). `get_track_cover(trackId)` — embedded art as base64, `null` → frontend gradient fallback.                                                                                                                                                                                                                                                                                                                                                                                                                 |

Wire types in `src/lib/models/` mirror the Rust structs via serde camelCase.

### Playback data flow

```
LibraryScreen/…  → playerStore.playQueue(tracks, i)
                    → invoke('audio_load_queue')
engine thread    → symphonia decode → rubato → SPSC ring
cpal callback    → 10×biquad (EQ) → volume+track gain → analyser tap → device
                    (the only realtime context: no alloc, no lock, no log)
engine → webview → Channel<EngineEvent>  (state, position, format, device)
                 → Channel<raw bytes>    (170-byte visual frame, ~60 Hz)
eqStore          → invoke('audio_set_eq')                (allowed edge eq → player)
WaveformCanvas   → visualizer.tick(frame, …)             (engine frame, or synthetic fallback)
RadioScreen      → playerStore.playStation(station)
                    → invoke('audio_play_stream')  (async: HTTP + prefetch off
                      the engine thread) → same decode/EQ/analyser path
```

## Svelte 5 Patterns

```typescript
let { prop1, prop2 = default }: Props = $props();  // always type with interface Props {}
let x = $state(initialValue)
let y = $derived(x.length)
let z = $derived.by(() => complexComputation())
$effect(() => { /* DOM side effects, rAF loops */ })
```

### State Management — pick the shape that matches the data

| Idiom                                                    | When to use                                                                                                                                                     | Examples                                                                      |
| -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| **Rune class with setter methods** in `*Store.svelte.ts` | Persistent or side-effecting state (IPC, the audio engine). Private `$state` + getters.                                                                         | `playerStore`, `eqStore`, `libraryStore`, `preferencesStore`, `languageStore` |
| **Rune class with public `$state` fields**               | Ephemeral state that must still survive leaving and returning to a screen (rule 12) — a selection, filter or view toggle with no persistence or IPC of its own. | `libraryViewStore`, `radioViewStore`, `nowPlayingViewStore`                   |
| **Bare `writable<T>`** in `*Store.ts`                    | Single primitive shared across screens for messaging.                                                                                                           | `searchQuery`, `eqOpen`                                                       |

- **`NavigationStore.svelte.ts`** is a getter over `$app/state.page` — a typed reactive view of the router. Never call `goto()` directly — `navigateTo()` is the chokepoint.

## Navigation

| Route          | Screen           |
| -------------- | ---------------- |
| `/now-playing` | NowPlayingScreen |
| `/library`     | LibraryScreen    |
| `/discover`    | DiscoverScreen   |
| `/radio`       | RadioScreen      |
| `/local`       | LocalFilesScreen |
| `/spotify`     | SpotifyScreen    |
| `/settings`    | SettingsScreen   |

See `docs/specs.md` for full screen behavior.

## Design System

Three layers with one-way dependencies between them (full methodology in [`ATOMIC_DESIGN.md`](ATOMIC_DESIGN.md)):

- `design-system/` → ∅ — atoms `IconButton`, `Chip`, `Badge`, `PrimaryButton`, `GhostButton`, `SectionLabel`, `Eyebrow`, `Toggle`, `ArtTile`, `EmptyState` (+ `IconButtonIdentifier` registry, `accent.ts` maps); molecules `NavItem`, `ToggleRow`, `PlayCircle`; organism `SidebarNav`.
- `features/<X>/` → `design-system/` first. Allowed cross-feature edges: `library → player`, `radio → player`, `settings → player` (every feature drives playback through the player's engine client), and the deliberately mutual pair `eq ↔ player` — the EQ store drives the engine's filters while the player's canvases and EQ affordance render EQ state; they are one signal path.
- `screens/` → any feature + design system. Screens are the only layer that freely crosses feature boundaries (e.g. NowPlayingScreen resolves library data and hands it to the player's queue card).

A grep for `design-system/` importing `features/`, or a feature-to-feature edge not on the list above, is a design smell — surface it.

SVG glyphs live as path data in `src/lib/icons/paths.ts` and render through `IconButton` / `NavItem` — no inline `<svg>` markup duplication for a glyph that exists there.
