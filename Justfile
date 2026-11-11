# Janis tasks. `just` lists them; `run` in the dev shell forwards to `just run`.
#
# Release recipes build the installers of one platform into dist/, with
# `cargo packager`: the Nix dev shell provides it on macOS, the other systems
# install it with `cargo install cargo-packager --locked`. Each is tied to its
# operating system, so run it there:
#
#   just release-macos     macOS, inside the Nix dev shell   → Janis.app + .dmg
#   just release-linux     Linux distro, outside Nix          → .AppImage, .deb, .rpm
#   just release-windows   Windows                            → NSIS and MSI installers

# List recipes.
default:
    @just --list

# Launch the app.
run:
    cargo run

# Format check, lints and tests: every quality gate.
check: fmt-rust-check clippy test-rust

test-rust:
    cargo test

clippy:
    cargo clippy --all-targets -- -D warnings

# Format the Rust sources (style in rustfmt.toml).
fmt-rust:
    cargo fmt

fmt-rust-check:
    cargo fmt --check

# The bundle is ad-hoc signed (`signing-identity` in Cargo.toml's packager
# section). Built in the dev shell, the binary links a library from the Nix
# store, which a signed bundle will not load and no other Mac has: it is
# pointed at the system's copy first, and the recipe fails if the bundle
# still reaches into the store or does not verify.
#
# macOS: build dist/Janis.app (signed) and its .dmg
[macos]
release-macos:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo packager --version >/dev/null 2>&1 || { echo "error: cargo-packager missing: enter the Nix dev shell (direnv allow, or nix develop)"; exit 1; }

    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
    case "$(uname -m)" in arm64) architecture=aarch64 ;; *) architecture=x64 ;; esac
    application=dist/Janis.app
    disk_image="dist/Janis_${version}_${architecture}.dmg"

    cargo build --release --locked
    scripts/macos/relink-system-libraries.sh target/release/Janis

    rm -rf dist
    cargo packager --release >/dev/null

    # The bundle must not reach back into the Nix store.
    if otool -L "$application/Contents/MacOS/Janis" | grep -q /nix/store; then
        echo "error: the bundle still references /nix/store:"
        otool -L "$application/Contents/MacOS/Janis" | grep /nix/store
        exit 1
    fi
    codesign --verify --deep --strict "$application"
    [ -f "$disk_image" ] || { echo "error: $disk_image was not built"; exit 1; }

    echo "Built $application ($(du -sh "$application" | cut -f1)) and $disk_image ($(du -h "$disk_image" | cut -f1))"

# Builds the release first. A running Janis is asked to quit, the .dmg is
# mounted out of sight, and the new app is copied next to the old one before
# replacing it, so a failed copy leaves the installed version in place.
#
# macOS: build the .dmg, then install its Janis.app into /Applications, replacing the old one
[macos]
install-macos: release-macos
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
    case "$(uname -m)" in arm64) architecture=aarch64 ;; *) architecture=x64 ;; esac
    disk_image="dist/Janis_${version}_${architecture}.dmg"
    target=/Applications/Janis.app
    executable="$target/Contents/MacOS/Janis"
    [ -f "$disk_image" ] || { echo "error: $disk_image not found"; exit 1; }
    [ -w /Applications ] || { echo "error: /Applications isn't writable from this account"; exit 1; }

    if pgrep -f "^$executable" >/dev/null; then
        echo "Quitting the installed Janis…"
        osascript -e 'tell application id "com.olivierbazin.janis" to quit' >/dev/null 2>&1 || true
        for _ in $(seq 50); do
            pgrep -f "^$executable" >/dev/null || break
            sleep 0.2
        done
        if pgrep -f "^$executable" >/dev/null; then
            echo "error: Janis is still running; quit it and run this again"
            exit 1
        fi
    fi

    mount=$(mktemp -d)
    # diskutil mounts the image where it has the "image" verb; hdiutil does
    # on systems without it, and for images of earlier releases, which embed
    # the license and ask for agreement before mounting: the "Y" answers
    # that, and is ignored otherwise.
    if ! { diskutil image attach --help >/dev/null 2>&1 \
        && diskutil image attach --readOnly --nobrowse --mountPoint "$mount" "$disk_image" >/dev/null 2>&1; }; then
        printf 'Y\n' | hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$disk_image" >/dev/null
    fi
    trap 'diskutil eject "$mount" >/dev/null 2>&1 || true; rmdir "$mount" 2>/dev/null || true' EXIT

    staging=/Applications/.Janis.app.installing
    rm -rf "$staging"
    ditto "$mount/Janis.app" "$staging"
    codesign --verify --deep --strict "$staging"
    rm -rf "$target"
    mv "$staging" "$target"

    # Spotlight and the Dock point at the new copy.
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$target"
    echo "Installed $target ($version)"

# Run on a regular distribution with the build dependencies the release
# workflow installs — not in the Nix dev shell, whose store-linked binaries
# would not run on other machines. The packager has no rpm format, so the rpm
# comes from `cargo generate-rpm` (`cargo install cargo-generate-rpm --locked`).
#
# Linux: build the AppImage, the deb and the rpm
[linux]
release-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "${IN_NIX_SHELL:-}" ]; then
        echo "error: run release-linux outside the Nix dev shell, on the distribution you build for"
        exit 1
    fi
    cargo build --release --locked
    rm -rf dist
    cargo packager --release
    cargo generate-rpm
    cp target/generate-rpm/*.rpm dist/
    ls -lh dist

# Windows: build the NSIS and MSI installers
[windows]
release-windows:
    cargo build --release --locked
    cargo packager --release
