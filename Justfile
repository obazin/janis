# Janis — task shorthands. `run` (the dev-shell wrapper) forwards here.

# Launch the app.
run:
    cargo run

# Every Rust gate: format, lints, tests.
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

# Build the installers of this platform into dist/ (needs `cargo install cargo-packager`).
# On macOS the binary is first pointed at the system's libraries: built in the
# dev shell it links some from the Nix store, which a signed bundle will not load.
release:
    cargo build --release
    if [ "$(uname)" = "Darwin" ]; then scripts/macos/relink-system-libraries.sh target/release/Janis; fi
    cargo packager --release
