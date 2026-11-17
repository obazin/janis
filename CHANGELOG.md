# Changelog

All notable changes to **Janis** are documented in this file. It is generated from the commit messages ([Conventional Commits](https://www.conventionalcommits.org)) by [git-cliff](https://git-cliff.org): edit commit messages, not this file.

## [v0.3.1](https://github.com/obazin/janis/releases/tag/v0.3.1) — 2026-11-17

Changes since [v0.3.0](https://github.com/obazin/janis/releases/tag/v0.3.0).

### Features

- **library:** Title an album's rows by their two-digit track number
- **library:** Star favourite songs, kept in step with the Jellyfin server
- **jellyfin:** Show or hide each Jellyfin library, with every one synced

### Fixes

- **jellyfin:** List a song once per library when its pages overlap

## [v0.3.0](https://github.com/obazin/janis/releases/tag/v0.3.0) — 2026-11-14

Changes since [v0.2.0](https://github.com/obazin/janis/releases/tag/v0.2.0).

### Features

- **library:** Search without regard to accents, as sorting already does
- **library:** Give a song without art its album card's gradient
- **radio:** Add stations from Morocco, Algeria and Tunisia
- **library:** Fold and unfold the Library's panels as the page glides to an opened tile
- **library:** Scroll an opened tile's row to the top of the page
- **player:** Draw the whole track in the mini player, as a DAW does
- **library:** List the Library's songs by artist, then album, then title
- **library:** List an artist's tracks album by album
- **library:** Sort the Library tabs alphabetically
- **jellyfin:** Show a server song's album picture before its own
- **jellyfin:** Add a Jellyfin server's songs to the library
- **ui:** Remove the Spotify placeholder screen
- **radio:** Let the listener star favourite radio stations

### Fixes

- **library:** Skip blank artists like missing ones, with the library view's remaining tests
- **library:** Keep initials to two letters and fold ø, ł and ß, with the text tests
- **jellyfin:** Sum up the Jellyfin card in its state module, and count its songs right
- **jellyfin:** Keep the current sign-in when signing in elsewhere fails
- **library:** Refuse a folder inside a watched one; let a parent take over its subfolders
- **library:** Follow links in watched folders, and count what a scan cannot enter
- **stations:** Count a pull's cap only over stations the catalog does not have
- **database:** Open a database a newer Janis wrote, without panicking in debug builds
- **player:** Start crossfade on before the preferences are read, as they say
- Refuse values that are not numbers before they reach the engine
- **library:** Tell albums apart by an unambiguous key
- Say "1 track" and "1 song", in each language's own way
- **ui:** Read a negative row from the window as no row, on every list
- **player:** Freeze the playhead where it was drawn when pausing
- **instance:** Let only one of two simultaneous launches become the instance
- Keep a background thread running after one of its jobs panics
- **jellyfin:** Refuse a listing that stops short of its total
- **ui:** Cancel the title bar's press once a window move begins
- **player:** Give every track a gain before it is first heard
- **jellyfin:** Give a bare Jellyfin host the server's default port

### Performance

- **covers:** Load covers only for tiles that stay in view, and prefetch beyond a resting list
- **player:** Measure loudness at the lowest priority
- **library:** Draw only the album and artist cards in view

### Refactor

- Move five small decisions out of the controller, each with a test
- Build the queue and the session's gains outside the controller
- **player:** Let the player say what the Now Playing lists depend on
- **radio:** Hold the Radio filters and build their chips in the radio module
- **library:** Build the Library selection and find its card in the library view
- **covers:** Decide which cover loads are still wanted outside the controller
- Decide the track overview's reads in a state module
- **jellyfin:** Decide where a track is read from in the Jellyfin module
- Decide what a click on the queue card does in the library view
- **now-playing:** Work out what Now Playing says in a pure module
- **jellyfin:** Share server pictures and one Jellyfin client

### Dependencies

- **deps:** Pin audio-stack-rs to its remote-tracks tag

## [v0.2.0](https://github.com/obazin/janis/releases/tag/v0.2.0) — 2026-11-11

Changes since [v0.1.0](https://github.com/obazin/janis/releases/tag/v0.1.0).

### Features

- Show a start failure in a message box
- **instance:** Have the running instance answer a second launch
- **instance:** Keep Janis to one running instance
- **ui:** Add the Discover, Settings and Spotify screens
- **ui:** Add the Radio screen
- **ui:** Add the Local Files screen and importing files dropped on the window
- **ui:** Add the Library screen
- **ui:** Add the equalizer sheet
- **ui:** Add the Now Playing screen
- **ui:** Open the window on a working shell: title bar, sidebar, mini player, audio
- **ui:** Add the Slint design system and a window to review it in

### Fixes

- Drop late results, fix the Now Playing lists and split the controller
- **library:** Keep the scanner faithful to what is on disk
- **covers:** Refuse cover art too large to decode safely
- **packaging:** Mount the macOS disk image with diskutil
- Keep the library accessor of the autoplay hook out of release builds
- **packaging:** Make locally built macOS bundles launch from Finder

### Performance

- **player:** Halve what playback costs the graphics processor

### Refactor

- **ui:** Take every radius and tint from the tokens
- Leave the equalizer's bounds and the language codes to their owners
- Remove the web frontend
- Detach the backend from Tauri and port the frontend logic to Rust

### Documentation

- Record the review's decisions and conventions
- Add path-scoped agent rules and a decision log
- Drop the web-era mechanics from the specifications
- Describe the application as it is now built

## [v0.1.0](https://github.com/obazin/janis/releases/tag/v0.1.0) — 2026-09-13

### Features

- **packaging:** Add a macOS installer script that clears the quarantine flag
- **library:** Expand a browsed album or artist inline under its tile
- **equalizer:** Offer the linear-phase mode, on audio-stack-rs spectral-dsp
- **radio:** Add Brazil to the catalog, deepening bossa nova
- **radio:** Deepen genre coverage (jazz, funk, blues, rock, pop, classical, bossa)
- **radio:** Make the country and genre filters multi-select
- **radio:** Drop country flags from the radio UI
- **radio:** Expand web radio to ~1000 stations and add a country filter
- **library:** Browse an album or artist instead of playing it on click
- **player:** Wire the gapless and crossfade toggles to real engine behaviour
- **radio:** Map the remaining Radio France stations to their metadata feeds
- **radio:** Reconnect to a station whose stream drops
- **audio:** Normalize playback volume, and fix the gain at gapless joins
- **audio:** Decode Opus with libopus
- **covers:** Show embedded cover art wherever a track or album appears
- **library:** Read full track metadata and show albums in their own order
- **radio:** Poll station now-playing APIs for artist, album and cover art
- **radio:** Parse ICY titles into artist, title and album
- **radio:** Expand the curated stations and improve genre filtering
- **now-playing:** Replace the spectrum with a queue card and artist spotlight

### Fixes

- Surface playback and cover-art failures instead of swallowing them
- **ui:** Make screen selections survive navigation, and codify it as a rule
- **player:** Fade before pausing, so pause-then-play-something-else doesn't click
- **player:** Let a reloaded webview rebuild its queue and station, and tidy the player UI
- **radio:** Constrain now-playing fetches, and refuse empty Opus packets
- **library:** Scan without starving the engine, prune only reachable roots, cap covers, and make migrations atomic
- **audio:** Follow the device on SetDevice, and stop rebuilding the resampler at every join
- **radio:** Guard the initial PlayStream by epoch, and keep a failed reconnect retrying
- **audio:** Keep the loudness meter away from radio, and measure gapless-advanced tracks
- **player:** Bound the failed-track advance, and let Play recover after the queue ends
- **audio:** Make the ring flush a real handshake, and fix the post-seek position
- **now-playing:** Stop the hero art collapsing to nothing

### Refactor

- **audio:** Extract the audio engine, radio and metadata into audio-stack-rs
- **audio:** Move decoding, DSP and output from the webview into Rust

### Documentation

- Align the living docs with the Rust audio engine

### Dependencies

- **deps:** Consume audio-stack-rs from GitHub instead of a local path


