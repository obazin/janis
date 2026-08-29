# Janis — task shorthands. `run` (the dev-shell wrapper) forwards here.

# Launch the app in dev with hot reload.
run:
    pnpm tauri dev

# Frontend type/lint gate.
check:
    pnpm check

# Unit tests (frontend).
test:
    pnpm test:unit

# Rust gates.
test-rust:
    cd src-tauri && cargo test

clippy:
    cd src-tauri && cargo clippy -- -D warnings

# Format the Rust sources (style in src-tauri/rustfmt.toml).
fmt-rust:
    cd src-tauri && cargo fmt

fmt-rust-check:
    cd src-tauri && cargo fmt --check

# Prettier over the frontend.
fmt:
    pnpm format

# Release bundle.
release:
    pnpm tauri build
