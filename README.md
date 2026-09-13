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
- **Settings** — playback switches (gapless, crossfade and normalization are live in the engine and on by default; exclusive output is persisted only), output info, language (English / Français).

Playlists, Spotify integration and user-added stations are on the roadmap.

## Install

Every release on the [releases page](https://github.com/obazin/janis/releases) carries a bundle per platform, built by the `release` GitHub Actions workflow. None of them is signed with a paid certificate, so each platform asks for one extra step the first time.

- **macOS** — the quickest route is the installer script, which downloads the DMG for your Mac, copies Janis into Applications and clears the quarantine flag, so the app opens with no prompt at all (re-run it to upgrade):

    ```bash
    curl -fsSL https://raw.githubusercontent.com/obazin/janis/main/scripts/macos/install.sh | sh
    ```

    Or download the DMG by hand (`Janis_x.y.z_aarch64.dmg` for Apple Silicon, `Janis_x.y.z_x64.dmg` for Intel) and drag Janis to Applications: the app is ad-hoc signed and not notarized, so the first launch then shows "Apple could not verify Janis is free of malware". Click **Done**, open **System Settings → Privacy & Security** and click **Open Anyway** next to Janis, or clear the flag once from a terminal: `xattr -dr com.apple.quarantine /Applications/Janis.app`.

- **Windows** (`Janis_x.y.z_x64-setup.exe`, or the `.msi`) — the installer is unsigned, so SmartScreen shows "Windows protected your PC". Click **More info → Run anyway**. The installer fetches the WebView2 runtime if the machine lacks it.
- **Linux** (x86_64 and arm64) — `.AppImage`: `chmod +x` and run (needs FUSE 2, `libfuse2` on Debian/Ubuntu). `.deb`: `sudo apt install ./Janis_x.y.z_amd64.deb`. `.rpm`: `sudo dnf install ./Janis-x.y.z-1.x86_64.rpm`. The packages depend on WebKitGTK 4.1, GTK 3 and ALSA.

- **Nix** (Linux and macOS) — the flake packages the app, built from source on your machine, so nothing is downloaded from the releases page and macOS never quarantines it:

    ```bash
    nix run github:obazin/janis              # try it without installing
    nix profile install github:obazin/janis  # install into your user profile
    ```

    In a NixOS, nix-darwin or Home Manager configuration, add `janis.url = "github:obazin/janis";` to the flake inputs and either list `inputs.janis.packages.${pkgs.system}.default` in the packages, or add `inputs.janis.overlays.default` to `nixpkgs.overlays` and use `pkgs.janis`. On macOS the package carries `Applications/Janis.app` next to the `Janis` command, which nix-darwin and Home Manager link where Spotlight finds it.

## Development

Requires [Nix](https://nixos.org) + [direnv](https://direnv.net) (the dev shell pins Rust, Node 22, pnpm and prettier, and carries the Linux GTK/webkit stack — see `flake.nix`).

```bash
direnv allow          # once — enters the dev shell automatically
pnpm install
pnpm tauri dev        # the app, with hot reload
```

Quality gates run locally (the only workflow in CI is the release build):

```bash
pnpm check            # svelte-check + TypeScript
pnpm test:unit        # vitest
just clippy           # cargo clippy -D warnings
just test-rust        # cargo test
just fmt-rust-check   # rustfmt verification
```

`just release` builds the distributable bundle for the current machine, ad-hoc signed on macOS. Publishing a release means bumping the version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `package.json` and `flake.nix`, then pushing a `vX.Y.Z` tag: the `release` workflow checks that the tag matches all four, builds every platform and opens a draft release holding the bundles.

`nix run` (or `nix build .#janis`) builds and launches the app through Nix alone — frontend and backend in a single derivation, no dev shell or `pnpm install` required.

## Architecture in one paragraph

Rust owns metadata, persistence and the filesystem: the scanner walks watched folders, reads tags with `lofty`, and upserts into a bundled-SQLite `janis.db`; the webview never touches the filesystem at all — audio decodes in Rust and cover art crosses IPC as base64, so Tauri's asset protocol is not even enabled. Rust owns sound: an engine thread decodes with symphonia, resamples only when the file and the device disagree, and feeds a lock-free ring that a cpal callback drains through ten peaking filters into the output device. The queue and the transport live there too, so playback is independent of the UI. Web radio takes the same path — buffered over HTTP into the same decoder — so stations get the equalizer and a real visualiser, and their ICY track titles show up in Now Playing. Frontend state lives in Svelte 5 rune-class stores hydrated once at boot; the UI is a three-layer atomic design system (`design-system/` → `features/` → `screens/`, one-way dependencies). Details in [`CLAUDE.md`](CLAUDE.md) and [`docs/specs.md`](docs/specs.md).

## License

[GPL-3.0](LICENSE). Fonts (Hanken Grotesk, Familjen Grotesk, Space Mono) are bundled under the SIL Open Font License — see `static/fonts/`.
