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
