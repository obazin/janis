---
paths:
  - "ui/**/*.slint"
---

# Slint UI: atomic design

## Tiers

Each component belongs to exactly one tier and only uses tiers below it. One component per file, in its tier's folder, with the file named after the component in snake case (`IconButton` → `atoms/icon_button.slint`). A component private to one file — a row of one list, a tab of one card — stays unexported in that file.

| Tier | Where | Components |
|---|---|---|
| Tokens and data | `tokens.slint`, `icons.slint`, `shell.slint` | `Tokens` (colours, gradients, shadows), `Fonts`, `Typography`, `Metrics` (radii, frame sizes), `Motion` (durations), `Accent`, `Icons`; `Shell` (open screen, search, equalizer sheet, notifications) |
| Atoms | `atoms/` | `Glyph`, `IconButton` (with the `IconButtonIdentifier` registry), `Chip`, `Badge`, `PrimaryButton`, `GhostButton`, `SectionLabel`, `Eyebrow`, `Toggle`, `ArtTile`, `EmptyState` |
| Molecules | `molecules/` | `NavItem`, `ToggleRow`, `PlayCircle`, `ScreenHeader`, `ScrollArea` |
| Organisms | `organisms/` | `SidebarNav`, `Toaster` |
| Features | `features/<name>/` | player: `MiniPlayer`, `Waveform`, `TransportControls`, `VolumeSlider`, `NowPlayingQueue` · equalizer: `EqualizerOverlay`, `EqualizerBandSlider`, `OpenEqualizerButton` · library: `CoverArt`, `AlbumCard`, `CardGrid`, `TrackRow`, `TrackList`, `LibraryTrackAccordion`, `LocalRow`, `LocalTable`, `ShelfTile`, `Shelf`, `ArtistSpotlight` · radio: `StationCard` |
| Screens | `screens/` | `AppTitlebar`, `NowPlayingScreen`, `LibraryScreen`, `DiscoverScreen`, `RadioScreen`, `LocalFilesScreen`, `SettingsScreen` |
| Page | `app.slint` | `AppWindow`: title bar, sidebar, the open screen, the mini player, and what floats above them |

Dependencies run one way. The design system (atoms, molecules, organisms) imports tokens and icons only and knows nothing about audio. A feature imports the design system; the allowed feature-to-feature edges are the pair `library ↔ player` (the artist spotlight plays through the player; the queue card shows library covers) and `player → equalizer` (the EQ buttons). Screens import anything. A design-system file importing from `features/`, or another feature edge, is a design smell: surface it.

## Data flow

- Rust and Slint meet only at the exported globals: `Shell`, `Player`, `Equalizer`, `Library`, `Covers`, `Radio`, `Settings`, `Strings` and `Tokens`. Each feature's global sits in its folder; the controller fills its properties and handles its callbacks.
- Design-system components take everything through `in` properties and report through callbacks. They never read a feature global or name a translation key: the caller passes `Strings.<key>`.
- Feature components read their feature's global and call its callbacks. They never compute what Rust already did: formatting, lists, counts and headings arrive ready.
- Screens lay features out and hold layout state only. A screen is created anew each time it is opened, so anything the listener should find again (a tab, a selection, a filter) lives in the controller.
- Which screen is open is interface state: `Shell.screen`, a string identifier the sidebar sets directly.
- Variants are an `enum` property (`BadgeVariant`, `PlayCircleVariant`), not a pile of booleans.

## Reuse first: search → ask → create

1. Search the tiers, then the feature's folder, for an existing component; use it as is.
2. A close match that needs a new property or variant changes a shared contract: ask before extending it.
3. Nothing fits: create it in its own file in the right tier and layer, with a comment saying what it is for. A pattern repeated three times inline is a missing atom.

## Accessibility

- Every interactive component has an `accessible-role`, an `accessible-label` and an `accessible-action-default` that does what a click does. Selectable ones (`Chip`, `NavItem`, `Toggle`, the queue tabs) also say `accessible-checkable` and `accessible-checked`; sliders give their minimum, maximum and value.
- An icon-only button carries its `IconButtonIdentifier` as `accessible-id`. Adding one means adding its entry to the registry.
- The `accessibility` Slint feature must stay enabled in `Cargo.toml`.

## Styling

- Only tokens: `Tokens.*`, `Fonts.*`, `Typography.*`, `Metrics.*`, `Motion.*`. No colour, typeface, text size, weight, radius, shadow or duration literal in a component. Padding, spacing and the size of a fixed box may be literal lengths. A circle or a pill takes its radius from its own size (`self.height / 2`), never from a number.
- One dark "prism" theme. A new colour is a new named token; a translucent tint of an accent is its own token (`row-hover`, `accent-ring`) or a function in `tokens.slint` (`accent-tint`, `play-pulse-at`), not `.with-alpha()` scattered through components.
- Icons are path data on a 24 × 24 grid in `icons.slint`, drawn with `Glyph` (stroked or filled); no path data in a component.
- Text comes from the `Strings` global (generated from `translations/en.json`) or from a feature global. A key missing from `en.json` does not compile. The only text written in a component is what no language translates: the names of the languages, each in its own, and the "JANIS" wordmark.

## Lists

- A list that can grow with the library draws only the rows in view. Rows have a fixed height; the screen's `ScrollArea` gives `scroll-offset`, and the list computes `first` and `count` and places each row at `index × row-height` (see `TrackList`, `LocalTable`, the Radio grid).
- A screen with several sections lays them out by position inside one `ScrollArea`, each section's `y` following the height of the one above, so a windowed list can know where its visible part starts.
- `FlexboxLayout` wraps (chips, presets); the other layouts do not.

## Animation and rendering cost

- The renderer repaints the whole window for any change. What an animation costs is its frame rate, not its size.
- Everything that moves during playback reads `Player.animation-time` (waveform, glow, pulse), which the controller ticks. Never use `animation-tick()`: it redraws at the display's rate.
- Short one-off transitions (hover, a toggle's knob) use `animate` with a `Motion` duration.
- After a change that animates, measure processor and graphics use before and after.

## Slint pitfalls in this codebase

- Element-level `opacity` on an animated item forces an offscreen layer (hundreds of megabytes): carry transparency in the colour.
- A stroked `Path` is laid out inside its stroke: grow it by the stroke width to keep it on its grid (`Glyph`, `Waveform`).
- A `Path` with a viewbox keeps its aspect ratio: give it a viewbox equal to its own size to fill a box.
- There is no blur and no dashed stroke. Soft edges are drawn into a picture (`halo.rs`); surfaces that would be translucent over a blur are nearly opaque.
- A layout cannot depend on a size it determines: bind to the parent's width, never to a value derived from the child's preferred size (the compiler reports a binding loop).
- An element with a width or height inside a plain (non-layout) parent is centred unless `x` and `y` are set.
- Some names cannot be property names: `color`, `row`, and a name that an enum value needs (`cover` shadows `ImageFit.cover`).
- `cache-rendering-hint` and the Skia renderer were both measured for this application and do not reduce what playback costs.

## Looking at the result

Compiling is not seeing. Debug builds save what the window shows: `JANIS_SNAPSHOT=<file.png>`, with `JANIS_SCREEN`, `JANIS_LIBRARY`, `JANIS_SEARCH`, `JANIS_EQUALIZER`, `JANIS_AUTOPLAY`, `JANIS_STATION` and `JANIS_FAVOURITES` to reach a state. Say so when something could not be checked this way (anything that needs a pointer: drags, hover, scrolling).
