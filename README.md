# Janis

Open-source desktop audio player. Local library with real metadata, curated web radio, a live 10-band equalizer, and live waveform visualisation — wrapped in a neon-on-violet "prism" interface.

Built with **Tauri v2** (Rust backend) and **Svelte 5 / SvelteKit 2 / Tailwind v4** (frontend). Audio decodes and plays in Rust — cpal for output, symphonia for decoding — through ten peaking filters and an analyser, so the EQ and the visualisers react to the actual signal. Named for Janis Joplin.

## Features

- **Now Playing** — hero album art (embedded cover art, or a generated prism gradient), track/artist/composer/album metadata, live waveform with click-to-seek, transport with shuffle/repeat, volume, a queue card (up next / playlist / current album), and an artist spotlight drawn from your library.
- **Library** — scan folders or import files; albums, artists and songs views grouped from real tags (`lofty`); search from the titlebar.
- **Discover** — shelves derived from your own library (recently added albums, artists, rediscoveries).
- **Web Radio** — curated station list (SomaFM, FIP, Radio Paradise, KEXP, …) with genre filters.
- **Local Files** — drop zone (real OS drag-and-drop), scanned-library table with format badges, watched-folder management, rescan.
- **10-band EQ** — bottom-sheet graphic equalizer with presets (Rock, Jazz, Bass Boost, …), applied live to playback, persisted across launches.
- **Settings** — playback switches (gapless, crossfade, normalization, exclusive output — persisted; wired up progressively), output info, language (English / Français).

Playlists, Spotify integration and user-added stations are on the roadmap.

## Development

Requires [Nix](https://nixos.org) + [direnv](https://direnv.net) (the dev shell pins Rust, Node 22, pnpm and prettier — see `flake.nix`).

```bash
direnv allow          # once — enters the dev shell automatically
pnpm install
pnpm tauri dev        # the app, with hot reload
```

Quality gates (no CI yet — run locally):

```bash
pnpm check            # svelte-check + TypeScript
pnpm test:unit        # vitest
just clippy           # cargo clippy -D warnings
just test-rust        # cargo test
just fmt-rust-check   # rustfmt verification
```

`just release` builds the distributable bundle.

## Architecture in one paragraph

Rust owns metadata, persistence and the filesystem: the scanner walks watched folders, reads tags with `lofty`, and upserts into a bundled-SQLite `janis.db`; the webview never touches the filesystem at all — audio decodes in Rust and cover art crosses IPC as base64, so Tauri's asset protocol is not even enabled. Rust owns sound: an engine thread decodes with symphonia, resamples only when the file and the device disagree, and feeds a lock-free ring that a cpal callback drains through ten peaking filters into the output device. The queue and the transport live there too, so playback is independent of the UI. Web radio takes the same path — buffered over HTTP into the same decoder — so stations get the equalizer and a real visualiser, and their ICY track titles show up in Now Playing. Frontend state lives in Svelte 5 rune-class stores hydrated once at boot; the UI is a three-layer atomic design system (`design-system/` → `features/` → `screens/`, one-way dependencies). Details in [`CLAUDE.md`](CLAUDE.md) and [`docs/specs.md`](docs/specs.md).

## License

[GPL-3.0](LICENSE). Fonts (Hanken Grotesk, Familjen Grotesk, Space Mono) are bundled under the SIL Open Font License — see `static/fonts/`.
