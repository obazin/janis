---
paths:
  - "src/**/*.rs"
  - "build.rs"
---

# Rust code

## Responsibilities and dependency direction

- `player.rs`, `equalizer.rs`, `playback_options.rs` and `notifications.rs` hold state and decide. They never import Slint, the engine, the database or a timer: a method updates the state and returns the `Effect`s (`effects.rs`) its decision implies. New behaviour goes there first, with a unit test, before it is wired to the window.
- `library_view.rs`, `radio.rs`, `text.rs`, `visualizer.rs`, `halo.rs` and `covers.rs` are pure: data in, data out. They take a `&Translations` when they format text.
- `application.rs` and `application/*.rs` are the only place that combines state, engine, database and window. A method there runs a state module's method, carries out the effects (`Application::run`), and refreshes what changed. It makes no decision a state module could make.
- `audio.rs` is the only file that names `audio_stack_rs` (see `.claude/rules/audio.md`). `library.rs` and `persistence.rs` are the only ones with SQL (see `.claude/rules/library.md`).
- `main.rs` only composes: logger, the one-instance claim, backend, window, controller, and the development hooks.
- What differs by operating system goes in `platform.rs` behind a neutral name, with `#[cfg(target_os = "…")]` inside.

## Threads

- The controller lives on the interface thread and is reached from other threads only through `application::post`. Nothing that belongs to the window crosses a thread.
- File reads, picture decoding and scans run on a `background::Worker` (`picture_worker`, `library_worker`); the station connect runs on `station_runtime`, which stays alive because the stream goes on downloading on it. A job ends by posting its result.
- A result that may arrive after the state moved on is recognised and dropped: it carries the epoch it was asked under (the current cover, a station's art, thumbnails, a station connection).
- Short SQLite writes of one preference are the one thing allowed on the interface thread.

## Window boundary

- Rust formats everything the window shows: times, labels, counts, headings. Slint receives strings and models and returns indices or identifiers, never re-derived values.
- Push what changed, at the rate it changes. Window callbacks end with `refresh_player` (scalars only); an action refreshes the lists it changed itself; `refresh_everything` is for a new library or another language. Setting an unchanged property is free, so a refresh may set everything it owns.
- A list for the window is one `Rc<VecModel<…>>` held by the controller and replaced with `set_vec`; the controller keeps what each row stands for, so a click is an index.

## Errors and feedback

- Diagnostics go through the `log` macros, never `println!` or `eprintln!` (`main.rs`'s fatal error and the development hooks excepted). `error` for what breaks a feature, `warn` for a skipped file or a failed action.
- A failure the listener should know about is an `Effect::NotifyError` (or `Application::notify_error`) with a translation key; the technical detail goes to the log. A burst of the same failure carries a dedupe key. Failing to save a preference goes through `Application::saved`.
- No `unwrap` on fallible runtime paths. `expect` only for programmer errors, with a message that says what was assumed.

## Testing

- Every state-module decision and every pure function has a test; the test names say the behaviour in a sentence.
- Catalog integrity is tested: both translation catalogs hold the same keys, and every station's genre and country has a label.
- `cargo test`; doctests are not used.

## Style

- Full words in every identifier, closure parameter and loop variable, except names imposed by an external interface. `library.rs` and `persistence.rs` predate the rule; use full words in what you add or rewrite there.
- Doc comments explain why and the contract; they describe the present.
