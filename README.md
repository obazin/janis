# Janis

Open-source desktop audio player. Local library with real metadata, curated web radio, a live 10-band equalizer, and live waveform visualisation — wrapped in a neon-on-violet "prism" interface.

A native application written in **Rust**, with a **[Slint](https://slint.dev)** interface. Audio decodes and plays in Rust — cpal for output, symphonia for decoding — through ten peaking filters and an analyser, so the EQ and the visualisers react to the actual signal. Named for Janis Joplin.

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

- **Windows** (`Janis_x.y.z_x64-setup.exe`, or the `.msi`) — the installer is unsigned, so SmartScreen shows "Windows protected your PC". Click **More info → Run anyway**.
- **Linux** (x86_64 and arm64) — `.AppImage`: `chmod +x` and run (needs FUSE 2, `libfuse2` on Debian/Ubuntu). `.deb`: `sudo apt install ./Janis_x.y.z_amd64.deb`. `.rpm`: `sudo dnf install ./Janis-x.y.z-1.x86_64.rpm`. The packages depend on ALSA, and on the fontconfig, xkbcommon and OpenGL libraries a desktop already has.

- **Nix** (Linux and macOS) — the flake packages the app, built from source on your machine, so nothing is downloaded from the releases page and macOS never quarantines it:

    ```bash
    nix run github:obazin/janis              # try it without installing
    nix profile install github:obazin/janis  # install into your user profile
    ```

    In a NixOS, nix-darwin or Home Manager configuration, add `janis.url = "github:obazin/janis";` to the flake inputs and either list `inputs.janis.packages.${pkgs.system}.default` in the packages, or add `inputs.janis.overlays.default` to `nixpkgs.overlays` and use `pkgs.janis`. On macOS the package carries `Applications/Janis.app` next to the `Janis` command, which nix-darwin and Home Manager link where Spotlight finds it.

## Development

Requires [Nix](https://nixos.org) + [direnv](https://direnv.net) (the dev shell pins the Rust toolchain and carries the libraries the window and the audio output need on Linux — see `flake.nix`).

```bash
direnv allow          # once — enters the dev shell automatically
cargo run             # the app (`just run`, or plain `run`, do the same)
```

Quality gates run locally (the only workflow in CI is the release build):

```bash
just check            # all three below
just fmt-rust-check   # rustfmt verification
just clippy           # cargo clippy -D warnings
just test-rust        # cargo test
```

`just release-macos` (or `release-linux`, `release-windows`) builds that platform's installers into `dist/`, ad-hoc signed on macOS; it needs [cargo-packager](https://github.com/crabnebula-dev/cargo-packager), which the Nix dev shell provides on macOS (elsewhere: `cargo install cargo-packager --locked`). `just install-macos` then replaces the Janis in `/Applications` with the one just built. Publishing a release means bumping the version in `Cargo.toml` and `flake.nix`, then pushing a `vX.Y.Z` tag: the `release` workflow checks that the tag matches both, builds every platform and opens a draft release holding the bundles.

`nix run` (or `nix build .#janis`) builds and launches the app through Nix alone, with no dev shell required.

## Architecture in one paragraph

One process, all Rust. The scanner walks watched folders, reads tags with `lofty`, and upserts into a bundled-SQLite `janis.db`. Sound is the `audio-stack-rs` crate's: an engine thread decodes with symphonia, resamples only when the file and the device disagree, and feeds a lock-free ring that a cpal callback drains through ten peaking filters into the output device. The queue and the transport live there too, so playback is independent of the UI. Web radio takes the same path — buffered over HTTP into the same decoder — so stations get the equalizer and a real visualiser, and their ICY track titles show up in Now Playing. What the screens show lives in plain state modules that decide and return effects; one controller on the interface thread carries the effects out and pushes what changed to the Slint globals the screens read. The interface (`ui/`) is a three-layer atomic design system (design system → features → screens, one-way dependencies) drawing every color, size and duration from one token file. Details in [`CLAUDE.md`](CLAUDE.md) and [`docs/specs.md`](docs/specs.md); the reasons behind them in [`decisions-log.md`](decisions-log.md).

## License

[GPL-3.0](LICENSE). Fonts (Hanken Grotesk, Familjen Grotesk, Space Mono) are bundled under the SIL Open Font License — see `ui/fonts/`.
