---
paths:
  - "src/audio.rs"
  - "src/player.rs"
  - "src/equalizer.rs"
  - "src/playback_options.rs"
  - "src/visualizer.rs"
  - "src/application.rs"
---

# The audio engine

- **Boundary.** The whole audio path — decode, DSP, output, transport, web radio, and audio-file metadata — lives in the `audio-stack-rs` crate, a git dependency pinned to a tag. `audio.rs` re-exports the facade items Janis uses; nothing else names `audio_stack_rs`, and no audio or metadata crate (cpal, symphonia, lofty, …) is ever a direct dependency. A behaviour the engine lacks is a change to that crate, not a workaround here.
- **Bumping the crate** means changing the tag in `Cargo.toml`, running `cargo update -p audio-stack-rs`, and regenerating `cargoHash` in `flake.nix`.
- **Events.** The engine calls `EngineBridge` from its own thread. `send_event` posts to the interface thread, where `Player::apply` takes the event into the mirror; `send_frame` only keeps the latest visual frame and never waits (a dropped frame is invisible, a stalled engine thread is audible).
- **The mirror is optimistic.** A click updates `Player` at once and the engine's next `State` event confirms or corrects it. The engine stays the authority: on radio, the station is recovered from the id the engine names.
- **Parameters are immediate, persistence is not.** Volume and EQ gains go straight to the engine so a slider is audible on the next buffer; the database write is a separate, debounced effect.
- **Stations.** `AudioEngine::play_stream` is the one asynchronous call. It runs on `station_runtime`, which must outlive the call: the stream downloads on the runtime that opened it.
- **The realtime callback** is the engine's; nothing in Janis runs there. Do not add work to `send_frame`.
- **Shutdown.** `Application::shut_down` stops the engine after the event loop ends, while the process is alive.
- **The overview.** `AudioEngine::read_overview` decodes a whole track, separately from playback, and blocks: call it only from the thread `application/track_overview.rs` starts for each read. A Jellyfin song is read from the server's small transcode (`Session::overview_location`), never a second full download of the original unless the server will not transcode.
- **The trace.** `Visualizer` turns the frame's 160 leading bytes into path commands sized to each waveform view. The analyser sits after the volume stage, so a muted engine draws a flat trace: test the trace with `JANIS_AUDIBLE` and the system output muted.
