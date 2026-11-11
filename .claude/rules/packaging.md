---
paths:
  - "flake.nix"
  - "Justfile"
  - "Cargo.toml"
  - "build.rs"
  - "packaging/**"
  - "scripts/macos/**"
  - ".github/workflows/**"
---

# Build, Nix and releases

- The Rust toolchain pin belongs to the `chess-flake` workspace; never add a `rust-toolchain.toml` or pin a compiler here. The Nix package is built with that toolchain (`makeRustPlatform`), because nixpkgs' own trails behind what Slint needs.
- After any change to `Cargo.lock` — a version bump is one — regenerate `cargoHash` in `flake.nix`: set it to `pkgs.lib.fakeHash`, run `nix build .#janis`, and copy the hash it reports.
- A Slint bump also moves `slintLanguageServer` in `flake.nix`: its version, its crate hash and its dependency hash, each found by setting it to `pkgs.lib.fakeHash` and reading what `nix develop` reports.
- The version is declared in `Cargo.toml` and `flake.nix`; bump both together. The release workflow refuses a tag that differs from either.
- A new native dependency is added in three places: the flake (the dev shell and the package), the Linux package list of `.github/workflows/release.yml`, and the README's Install section if it changes what a machine needs.
- Installers come from `cargo packager`, configured in `Cargo.toml`. On macOS the dev shell builds it (`cargoPackager` in `flake.nix`, a pinned version with its own two hashes); the other systems and the release workflow `cargo install` it. It has no rpm format: `cargo generate-rpm` builds that one. Release recipes are gated with `[macos]`, `[linux]`, `[windows]` and write to `dist/`.
- After changing `release-macos`, run it: it must report no reference to `/nix/store` and pass `codesign --verify --deep --strict`. Then open the bundle through Finder (`open dist/Janis.app`), not only from a terminal — a signed bundle refuses libraries a bare binary loads.
- The disk image embeds no license agreement: mounting one would wait for a click.
- Artifact names (`Janis_<version>_<arch>.dmg`, …) are what `scripts/macos/install.sh` and the release notes expect; changing them means changing those.
- `packaging/macos/Info.plist` is the Nix package's bundle (the packager writes its own); check it with `plutil -lint`.
- `build.rs` compiles `ui/app.slint` with embedded resources and generates the `Strings` global from `translations/en.json`.
- The release workflow has never run for this stack; treat its first run as a test.
