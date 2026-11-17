# Janis tasks. `just` lists them; `run` in the dev shell forwards to `just run`.
#
# `just release <level>` cuts a version: it bumps Cargo.toml, Cargo.lock and
# flake.nix (and the flake's vendor hash), writes CHANGELOG.md with git-cliff
# from the commit messages, then commits, tags and pushes. The tag starts the
# `release` workflow, which builds every platform and opens a draft release
# with that version's changelog section; publishing the draft stays manual.
# Run it inside the Nix dev shell, which carries git-cliff and cargo-edit.
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
check: fmt-rust-check clippy test-rust check-stations

# The cargo arguments of the catalog builder, a package of its own.
stations_package := "--manifest-path scripts/radio/Cargo.toml"
stations_build := stations_package + " --target-dir target/tools"

# Rebuild data/stations.json from the curated seed and Radio Browser (network).
stations *arguments:
    cargo run {{ stations_build }} -- {{ arguments }}

# The same gates for the catalog builder.
check-stations:
    cargo fmt {{ stations_package }} --check
    cargo clippy {{ stations_build }} --all-targets -- -D warnings
    cargo test {{ stations_build }}

test-rust:
    cargo test

clippy:
    cargo clippy --all-targets -- -D warnings

# Format the Rust sources (style in rustfmt.toml).
fmt-rust:
    cargo fmt
    cargo fmt {{ stations_package }}

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

# --- Versioned releases ------------------------------------------------------

# LEVEL is `patch`, `minor` or `major`:
#   just release patch     # 0.3.0 -> 0.3.1
#   just release minor     # 0.3.0 -> 0.4.0
#   just release major     # 0.3.0 -> 1.0.0
# Shows the notes and waits for a yes before anything leaves the machine; a no,
# or a failure before the commit, puts the four files back as they were.
#
# Bump the version, write the changelog, commit, tag and push (the workflow drafts the release)
release level: _require-clean
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ level }}" in
      patch|minor|major) ;;
      *) echo "error: level must be patch, minor or major (got '{{ level }}')" >&2; exit 2 ;;
    esac
    for tool in git-cliff nix; do
      command -v "$tool" >/dev/null || { echo "error: $tool missing: enter the Nix dev shell (direnv allow, or nix develop)" >&2; exit 1; }
    done
    cargo set-version --help >/dev/null 2>&1 || { echo "error: cargo set-version missing: enter the Nix dev shell" >&2; exit 1; }

    # Releases are cut from main: the workflow builds the tag, and the
    # changelog reads the history behind it.
    branch="$(git rev-parse --abbrev-ref HEAD)"
    if [ "$branch" != "main" ] && [ "${ALLOW_RELEASE_BRANCH:-}" != "1" ]; then
      echo "error: refusing to release from '$branch' (expected main); set ALLOW_RELEASE_BRANCH=1 to override" >&2
      exit 1
    fi

    restore() {
      git checkout -- Cargo.toml Cargo.lock flake.nix CHANGELOG.md 2>/dev/null || true
    }
    trap 'echo "Failed: the version bump is undone." >&2; restore' ERR

    # 1. The version, in Cargo.toml and Cargo.lock, then in the flake's
    #    janis package (the release workflow refuses a tag either disagrees with).
    cargo set-version --bump "{{ level }}"
    version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
    tag="v${version}"
    # Values reach perl through the environment: a base64 hash holds slashes.
    VALUE="$version" perl -0pi -e 's/(pname = "janis";\s*version = ")[^"]*"/$1$ENV{VALUE}"/' flake.nix
    flake_version="$(awk '/pname = "janis";/ { found = 1 } found && /version = "/ { gsub(/.*version = "|";.*/, ""); print; exit }' flake.nix)"
    [ "$flake_version" = "$version" ] || { echo "error: flake.nix still says $flake_version" >&2; false; }
    echo "▶ Preparing $tag on $branch"

    # 2. The changelog, whole; the notes are this version's section without
    #    its heading, which the release title already carries.
    git cliff --tag "$tag" --output CHANGELOG.md 2>/dev/null
    notes="$(git cliff --tag "$tag" --unreleased --strip all 2>/dev/null | grep -v '^## \[' | sed '/./,$!d')"

    echo "──────── release notes for $tag ────────"
    echo "$notes"
    echo "────────────────────────────────────────"
    reply=""
    read -r -p "Release $tag with these notes? [y/N] " reply || true
    if [[ "$reply" != "y" && "$reply" != "Y" ]]; then
      echo "Aborted: the version bump is undone."
      trap - ERR
      restore
      exit 1
    fi

    # 3. Cargo.lock carries the package's own version, so the flake's vendor
    #    hash moves with it: ask Nix for the new one, then build the package
    #    with it.
    echo "▶ Computing the flake's vendor hash (nix build)"
    fake_hash="sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    VALUE="$fake_hash" perl -0pi -e 's/(pname = "janis";.*?cargoHash = ")[^"]*"/$1$ENV{VALUE}"/s' flake.nix
    cargo_hash="$(nix build .#janis.cargoDeps --no-link 2>&1 | sed -n 's/^ *got: *\(sha256-[^ ]*\)$/\1/p' | head -1 || true)"
    [ -n "$cargo_hash" ] || { echo "error: Nix reported no vendor hash" >&2; false; }
    VALUE="$cargo_hash" perl -0pi -e 's/(pname = "janis";.*?cargoHash = ")[^"]*"/$1$ENV{VALUE}"/s' flake.nix
    echo "▶ Building the Nix package"
    nix build .#janis --no-link

    # 4. Commit and an annotated tag; nothing can fail back into the trap
    #    after this.
    trap - ERR
    git add Cargo.toml Cargo.lock flake.nix CHANGELOG.md
    git commit -q -m "chore(release): $tag"
    git tag -a "$tag" -m "Janis $version"

    # 5. Push the branch and the tag: the tag starts the release workflow.
    git push --follow-tags
    echo "✔ Pushed $tag. The release workflow builds the installers and opens a draft:"
    echo "    https://github.com/obazin/janis/actions/workflows/release.yml"
    echo "  Publish it from https://github.com/obazin/janis/releases once every installer is attached."

#   just release-preview minor
#
# Preview the notes of the next release at LEVEL, changing nothing
release-preview level:
    #!/usr/bin/env bash
    set -euo pipefail
    current="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
    IFS='.' read -r major minor patch <<< "$current"
    case "{{ level }}" in
      patch) patch=$((patch + 1)) ;;
      minor) minor=$((minor + 1)); patch=0 ;;
      major) major=$((major + 1)); minor=0; patch=0 ;;
      *) echo "error: level must be patch, minor or major (got '{{ level }}')" >&2; exit 2 ;;
    esac
    next="${major}.${minor}.${patch}"
    echo "Next {{ level }} version: $next (from $current)"
    git cliff --tag "v$next" --unreleased --strip all 2>/dev/null | grep -v '^## \[' | sed '/./,$!d'

# Regenerate CHANGELOG.md from the whole history (no bump, no release).
changelog:
    git cliff --output CHANGELOG.md

# Fail when the working tree has uncommitted changes.
_require-clean:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "$(git status --porcelain)" ]; then
      echo "error: the working tree is not clean: commit or stash first" >&2
      exit 1
    fi
