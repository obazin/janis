# Atomic Design — Concepts & Methodology

> A practitioner's reference for Atomic Design, distilled from Brad Frost's
> _Atomic Design_ (Chapter 2) and translated to **Svelte 5 + SvelteKit 2**
> idioms. This document is the conceptual companion to Janis's living
> design system at `src/lib/design-system/` (generic, domain-agnostic
> primitives) and the per-feature domain code at `src/lib/features/`
> (audio-player components built on top of the design system). Atomic
> tiers (atoms / molecules / organisms) apply equally to both layers.

---

## 1. What Atomic Design Is

> _"Atomic design is a methodology composed of five distinct stages working
> together to create interface design systems in a more deliberate and
> hierarchical manner."_ — Brad Frost

Atomic Design is **not** a folder layout, a framework, a CSS architecture, or
a rendering technique. It is a **mental model** for thinking about user
interfaces as **both a cohesive whole and a collection of parts at the same
time**. The five stages give the vocabulary; the rest is judgement.

Two consequences follow immediately:

- The methodology is independent of platform (web, native, embedded, print).
  "Atomic design has nothing to do with web-specific subjects like CSS or
  JavaScript architecture."
- The stages are **categories of thought**, not a build order. You do not
  finish atoms, then start molecules. You move freely between scales.

---

## 2. The Chemistry Analogy

Frost borrows from natural science:

- **Atoms** — irreducible primitives. In chemistry: hydrogen, oxygen. In UI:
  a label, an input, a button.
- **Molecules** — small groups of atoms bonded into a single functional
  unit. In chemistry: H₂O. In UI: a search form (label + input + button).
- **Organisms** — larger assemblies of molecules (and/or atoms, and/or other
  organisms) that form a distinct section of an interface. Think of a cell
  versus a water molecule — both useful, at different scales.
- **Templates** — the skeleton of a page; layout without final content.
- **Pages** — a template populated with real, representative content.

The analogy is a hierarchy aid, not a science lesson. Frost's point is that
**deliberate composition** at each level creates predictable, testable systems.

---

## 3. The Five Stages

### 3.1 Atoms

The foundational building blocks. **Atoms cannot be broken down further
without losing function.**

| Trait        | Detail                                                     |
| ------------ | ---------------------------------------------------------- |
| Examples     | Button, Input, Label, Icon, Badge, Tooltip-wrapper, Avatar |
| State        | Usually **stateless**; visual state comes from props       |
| Side effects | None. No persistence, no IPC, no router calls              |
| Knowledge    | Knows nothing about its caller, its screen, or the domain  |
| Reuse        | Reused everywhere — must stay generic                      |

**Rule of thumb.** If you find yourself adding domain vocabulary
(`trackId`, `eqPreset`, `stationUrl`) to an atom, it has stopped being an atom.

**Common trap — over-abstracting atoms.** A button with twelve booleans for
every possible future variant is harder to use than three buttons with clear
intent (`PrimaryButton`, `GhostButton`, `IconButton`). Prefer **a small set
of named variants** over a single mega-component with combinatorial props.

### 3.2 Molecules

> _"Relatively simple groups of UI elements functioning together as a unit."_

A molecule is the **smallest unit that does something meaningful**. It
applies the single-responsibility principle at the UI level: one molecule,
one job.

| Trait        | Detail                                                                                         |
| ------------ | ---------------------------------------------------------------------------------------------- |
| Examples     | SearchField (Input + IconButton), PlayerRow (Sticker + Name + Elo), MetricCard (Label + Value) |
| State        | Usually **stateless** or owns trivial local state (a hover toggle)                             |
| Side effects | None                                                                                           |
| Composition  | Made of atoms; may compose smaller molecules                                                   |

If a "molecule" starts orchestrating data fetching or routing, it has likely
crossed into organism territory — promote it.

### 3.3 Organisms

> _"Relatively complex UI components composed of groups of molecules and/or
> atoms and/or other organisms."_

Organisms form **distinct interface sections**: a header, a sidebar, a
toolbar, a results list. They are the first level where domain meaning is
allowed to leak in.

| Trait        | Detail                                                       |
| ------------ | ------------------------------------------------------------ |
| Examples     | Header (Logo + Nav + SearchField), SidebarNav, MiniPlayer    |
| State        | May own local UI state (open/closed, selected tab)           |
| Side effects | May read stores; should not perform persistence directly     |
| Composition  | Combines molecules + atoms; can repeat one molecule (a list) |

A useful test: **an organism can stand alone in a documentation page** and
still make visual sense. Templates and pages are where they get arranged.

### 3.4 Templates

> _"Page-level objects that place components into a layout and articulate
> the design's underlying content structure."_

A template defines **two things at once**: the spatial scaffolding ("here
goes the sidebar, here goes the main content, here goes the toolbar") and
the **shape of the content** those regions are expected to carry — image
dimensions, character-length ranges for headings, how many items a list
tolerates before it needs pagination. It is a wireframe in the sense that
final content is absent, but it is **not purely a layout concern**: a
template that says "header region, 200×60px logo, nav with up to 6 items,
a headline of 40–80 characters" is doing template work; a template that
says only "header region, main region, footer region" is underspecified.

In Svelte 5 those regions are expressed either as `children` / named
snippet props (`{@render header()}`, `{@render main()}`) or, at the route
level, as the implicit child rendered by a `+layout.svelte`.

| Trait           | Detail                                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------------------- |
| Examples        | TwoColumnNowPlayingTemplate, DashboardTemplate, ModalShellTemplate                                      |
| Content         | Placeholder / lorem / mocked data                                                                       |
| Concerns        | Layout, spacing, responsive behaviour **and** content-shape constraints (sizes, lengths, cardinalities) |
| Not its concern | Real data binding, business logic, final copy                                                           |

In SvelteKit terms, **`+layout.svelte` files are template-shaped**: they
own the scaffolding that screens slot into.

### 3.5 Pages

> _"Specific instances of templates that show what a UI looks like with real
> representative content in place."_

Pages are where the design system meets reality. They are also **the
stress test**: if a real track title is 80 characters long, a real album tag is
missing, a real folder holds ten thousand files — does the
template still hold?

| Trait    | Detail                                                    |
| -------- | --------------------------------------------------------- |
| Examples | `/now-playing` NowPlayingScreen, `/library` LibraryScreen |
| Content  | Real, representative — including edge cases               |
| Purpose  | Validate the system; surface variants the template missed |

When a page reveals that real content **breaks the pattern**, the action is
to go back down the hierarchy — fix the template, or the organism, or the
molecule — not to patch it at the page level.

---

## 4. Core Principles

### 4.1 It is not a linear process

> _"Atomic design is not a linear process, but rather a mental model to help
> us think of our user interfaces as both a cohesive whole and a collection
> of parts at the same time."_

You will move up and down the hierarchy continuously. Building a page may
reveal a missing atom; building an atom may suggest a new molecule. **Do not
finish a level before starting the next** — concurrently design the system
and the surfaces that use it.

### 4.2 The painter's analogy

Frost cites Frank Chimero: a painter steps back to assess the whole canvas,
then steps forward to refine a single brushstroke. Atomic Design gives the
same **zoom in / zoom out** discipline for UI work. The biggest practical
advantage is the ability to **shift quickly between abstract and concrete**.

### 4.3 Content drives structure

A design system must **cater to the content that lives inside it**, not the
other way around. Templates articulate the structure; pages reveal whether
the structure can carry real load. If real content breaks the pattern, the
pattern is wrong.

### 4.4 The taxonomy is flexible

> _"Atomic design is not rigid dogma."_

Frost notes that GE Design used "Principles, Basics, Components, Templates,
Features, Applications" instead of atoms/molecules/etc. — and that was
fine. What matters is that **the hierarchy is clear, shared, and useful for
your team**. Bikeshedding the name of a tier is wasted energy.

### 4.5 Universality

The methodology applies to **any** user interface — web, native, embedded.
It has no opinion on your CSS architecture, your bundler, your state
manager. Those are orthogonal decisions.

---

## 5. Common Challenges (and How to Avoid Them)

### 5.1 Categorisation fatigue

**Symptom.** The team spends 30 minutes debating whether
`CheckoutProgressIndicator` is a molecule or an organism.

**Cure.** Decide as a team **once**, write it down, move on. The cost of
mis-categorising a component is low; the cost of a long debate is high.
When in doubt, ship it as a molecule and promote it later if it grows.

### 5.2 Over-engineered atoms

**Symptom.** A `<Button>` with 14 props, 5 variants, an internal state
machine, and a `onLongPress` handler "just in case".

**Cure.** Atoms should be **embarrassingly simple**. Add a prop the day a
second caller actually needs it — not before. Three near-identical buttons
are cheaper than one omnipotent button that no one fully understands.

### 5.3 State leakage and the "where does state live?" problem

**Symptom.** A molecule pulls from a global store; an atom subscribes to
the router; persistence calls appear inside a button.

**Cure.** **Push state up.** Atoms and molecules receive data via props.
State lives in **organisms and above**, or in stores. The lower in the
hierarchy a component sits, the less it should know.

### 5.4 Component proliferation

**Symptom.** Five subtly different "card" components, three "header"
variants, two competing button systems.

**Cure.** Audit periodically. Before creating a new component, **search the
existing inventory first**. If a pattern appears three times inline,
extract it into a real component.

### 5.5 Scaling friction

**Symptom.** What worked at 30 components becomes opaque at 300.

**Cure.** Maintain a **single inventory** so the system is browsable — for Janis, at its current size, that inventory is the Design System section of `CLAUDE.md`, which names every atom, molecule and organism. Treat new components as **internal-first** — let them prove they belong before promoting them into the public design-system layer.

---

## 6. Best Practices (Svelte 5 Edition)

### 6.1 Folder structure

Mirror the hierarchy on disk. **Janis separates UI into three layers**:
the design system (primitives), features (domain capabilities), and
screens (compositions). Atomic tiers (atoms / molecules / organisms)
are a classification scheme that applies inside both the design system
and each feature folder:

```
src/lib/
  design-system/                          # THE design system — could ship as a UI library.
    atoms/      molecules/   organisms/   # Domain-agnostic primitives. Check here first.

  features/                               # Capability-shaped domain code (NOT screen-shaped).
    player/                               # Engine mirror + transport + visualisers
      PlayerStore.svelte.ts               # Feature owns its state (a mirror of the Rust engine).
      audioEngine.ts  visualizer.ts       # IPC client for the Rust engine + shared frame data
      MiniPlayer.svelte  WaveformCanvas.svelte  NowPlayingQueue.svelte  …
    eq/                                   # 10-band EQ
      EqStore.svelte.ts  presets.ts  bands.ts
      EqualizerOverlay.svelte  EqBandSlider.svelte  OpenEqButton.svelte
    library/                              # Track library (scan, folders, grouping)
      LibraryStore.svelte.ts
      TrackRow.svelte  LocalRow.svelte  AlbumCard.svelte  ShelfTile.svelte
      CoverArt.svelte  ArtistSpotlight.svelte
    radio/                                # Curated web-radio stations
      stations.ts  StationCard.svelte
    settings/                             # Playback preferences
      PreferencesStore.svelte.ts

  screens/                                # Compositions — pages in atomic-design terms.
    NowPlayingScreen.svelte               # Each cherry-picks from one or more features
    LibraryScreen.svelte                  # + the design system. A screen is NOT a feature.
    SettingsScreen.svelte  …

  models/ stores/ i18n/ icons/            # Cross-cutting infrastructure.

src/routes/                               # SvelteKit routes — 1-line wrappers around screens.
```

**Dependency direction between the layers is one-way:**

- `design-system/` depends on nothing.
- `features/<X>/` depends on `design-system/` first; allowed cross-feature edges (kept minimal): `library → player`, `radio → player`, `settings → player` — every feature drives playback through the player's engine client — plus the deliberately mutual pair `eq ↔ player`, because the EQ and the player describe one signal path (the EQ store drives the engine's filters; the player's canvases and EQ affordance render EQ state).
- `screens/` depends on `features/<any>` + `design-system/`. Screens are the only layer that freely crosses feature boundaries — NowPlayingScreen, for instance, resolves library data and hands it to the player's queue card, so the player never imports the library.

This separation is what makes the design system extractable and what
gives "reuse-first" a physical meaning (scan `design-system/` first,
then the relevant feature folders). It also matches the deeper insight
that a screen is a _composition_, not a _feature_ — a screen exists
to wire features together into a user-facing experience.

### 6.2 Naming conventions

- **PascalCase** for components (`IconButton.svelte`, `PlayerRow.svelte`).
- **Suffix by role** for non-components (`PlayerStore.svelte.ts`, `EqOverlayStore.ts`); a store holding `$state` takes the `.svelte.ts` extension.
- **One responsibility per file name** — if the name needs `And` or `Or`,
  the component is doing too much.
- **Co-locate** the `.figma.ts` Code Connect mapping next to the component
  (`PillLabel.svelte` + `PillLabel.figma.ts`).

### 6.3 Props and component APIs

Keep APIs **decoupled and generic** — components should solve problems
dynamically, not be wired to a fixed display.

```svelte
<!-- atom: knows nothing of its caller -->
<script lang="ts">
    interface Props {
        label: string;
        onclick: () => void;
        variant?: 'primary' | 'ghost';
        disabled?: boolean;
    }
    let { label, onclick, variant = 'primary', disabled = false }: Props = $props();
</script>
```

Rules:

- **Always type props with an `interface Props`** (Svelte 5 idiom).
- **Callbacks, not events** — pass `onSelect: (id) => void` as a prop
  rather than calling `createEventDispatcher` (Svelte 4 legacy). This is
  the official Svelte 5 migration guidance — components communicate
  upward by invoking callback props, not by dispatching custom events.
- **Snippets replace slots in Svelte 5.** Content passed inside a component
  tag becomes the implicit `children` snippet, rendered with
  `{@render children()}`. Type it as `children: Snippet` (or
  `Snippet<[ArgType]>` when the snippet takes arguments).
- **No `export let`** — use `$props()`.

### 6.4 State ownership

Where state lives matters more than how it's implemented:

| Level                 | What state it owns                               |
| --------------------- | ------------------------------------------------ |
| Atoms                 | None (or trivial visual toggles)                 |
| Molecules             | Local UI state at most (hover, focus)            |
| Organisms             | Section-level UI state (open tab, selected item) |
| Stores (`.svelte.ts`) | Persistent or cross-screen state                 |
| Templates / Layouts   | Boot-time wiring (`bootPromise`)                 |
| Pages / Screens       | Compose stores + organisms; thin orchestration   |

This is the Svelte translation of "manage state at higher levels and pass it down via props". For Janis specifically, see CLAUDE.md rule 13 for the store-shape policy (setter methods for persistent state, public `$state` fields for ephemeral state).

### 6.5 Reactivity primitives (Svelte 5 runes)

Each tier of the atomic hierarchy should reach for the simplest rune that
covers its needs:

| Rune                           | Purpose                                                                                                                         | Typical tier                         |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| `let x = $state(initial)`      | Reactive local state. Mutate by assignment.                                                                                     | Atoms (rarely), molecules, organisms |
| `let y = $derived(expr)`       | Pure computation derived from state — must be side-effect free.                                                                 | Anywhere `$state` is used            |
| `let z = $derived.by(() => …)` | Same as `$derived`, but with a function body for non-trivial expressions.                                                       | Organisms, screens                   |
| `$effect(() => { … })`         | DOM-side effects, subscriptions, imperative APIs. Returns an optional cleanup.                                                  | Organisms, screens, stores           |
| `setContext` / `getContext`    | Pass values down a subtree without prop drilling — typed via a `Symbol` key.                                                    | Templates / organism trees           |
| Stores in `*.svelte.ts` files  | Cross-screen reactive state with persistence or side effects. Setter methods are the chokepoint for writes (see Janis rule 13). | Above the screen layer               |

**On memoisation.** Svelte's compiled reactivity tracks dependencies at the
expression level and updates only the DOM nodes that depend on a changed
signal — there is no virtual DOM diff and no per-render component
re-execution to worry about. Reach for `$derived` to **share computed
values** between consumers, not to "prevent re-renders". Reach for
`$effect` only when you need a true side effect (DOM measurement, event
subscription, imperative library API) — never to mirror state.

### 6.6 Documentation and isolation

Atoms and molecules should be developed and reviewed **in isolation**, not
discovered for the first time inside a page. In a Svelte project the two
tools that fit this role are:

- **Histoire** — Svelte-native story runner, fastest to set up.
- **Storybook (Svelte adapter)** — heavier, but useful if the team already
  knows it from other stacks.

Whichever you pick, the discipline is the same:

- One story per meaningful variant (states and sizes — Janis ships a single dark "prism" theme, so there are no theme variants to cover; see CLAUDE.md rule 4).
- Cover edge cases (long text, empty state, error state, RTL) before the
  component lands in a page.
- Co-locate stories with the component file so they evolve together.

Until a story runner is wired up, a temporary `/dev/components` route that
renders each atom and molecule with sample inputs is a perfectly adequate
substitute.

### 6.7 Validation against design

Whenever a component originates from Figma, the design system layer is **the contract** between visual intent and code. Use Code Connect mappings (`<Component>.figma.ts`) so the Figma MCP server returns the _mapped_ component rather than re-deriving markup. (Janis has no Figma source today; this applies if one is introduced.)

---

## 7. Worked Examples

### 7.1 Decomposing an e-commerce header

| Tier      | Element                                                                                             |
| --------- | --------------------------------------------------------------------------------------------------- |
| Atoms     | Logo, Input, IconButton (search), NavLink, CartIcon, Badge (count)                                  |
| Molecules | SearchField (Input + IconButton), NavItem (NavLink + optional Badge), CartButton (CartIcon + Badge) |
| Organism  | Header (Logo + Nav (× NavItem) + SearchField + CartButton + UserMenu)                               |
| Template  | StoreLayout — snippet regions for header, body, footer                                              |
| Page      | `/products/[slug]` — real product data                                                              |

Update one atom (say, `IconButton`) and **every consumer benefits**. That
is the practical payoff.

### 7.2 Decomposing a dashboard

| Tier      | Element                                                                                                  |
| --------- | -------------------------------------------------------------------------------------------------------- |
| Atoms     | Label, Number, Icon, Button, ToggleSwitch                                                                |
| Molecules | MetricCard (Label + Number), FilterDropdown (Button + menu), ToolbarButton                               |
| Organisms | Sidebar (Nav + UserChip), ChartPanel (FilterDropdown + Chart + export Button), MetricGrid (× MetricCard) |
| Template  | DashboardLayout — sidebar + main + toolbar snippet regions                                               |
| Page      | `/dashboard` — real metrics, real user, real filters                                                     |

The same `DashboardLayout` template can host analytics, user management, or
sales pages just by swapping the organisms rendered into each region.

---

## 8. How Janis Applies This

Janis separates **the design system** (generic, domain-agnostic primitives
under `src/lib/design-system/`) from **the application** (audio-player
domain code under `src/lib/features/`). Atomic-design tiering classifies
components inside both layers.

### The design system (`src/lib/design-system/`)

Could plausibly ship as a separate UI library — nothing here knows
about audio.

- **Atoms** — `IconButton`, `Chip`, `Badge`, `PrimaryButton`,
  `GhostButton`, `SectionLabel`, `Eyebrow`, `Toggle`, `ArtTile`,
  `EmptyState`. Plus the canonical-registry enum `IconButtonIdentifier`
  and the shared `accent.ts` maps.
- **Molecules** — `NavItem`, `ToggleRow`, `PlayCircle`.
- **Organisms** — `SidebarNav`.

### The features (`src/lib/features/`)

Each feature folder is **capability-shaped** — a coherent unit of
functionality that screens can compose. Features own their stores +
tier files. Features do **not** own screens.

- **`player/`** — the engine mirror. Store: `PlayerStore.svelte.ts` (queue, transport, volume — a reactive mirror of the Rust engine, which owns the actual audio). Infrastructure: `audioEngine.ts` (the IPC client for the Rust engine), `visualizer.ts` (shared per-frame visual data), `format.ts`. Components: `MiniPlayer`, `WaveformCanvas`, `NowPlayingQueue`, `TransportControls`, `VolumeSlider`.
- **`eq/`** — the 10-band EQ. Store: `EqStore.svelte.ts`. Data: `presets.ts`, `bands.ts`. Components: `EqualizerOverlay` (bottom sheet), `EqBandSlider`, `OpenEqButton`.
- **`library/`** — the track library. Store: `LibraryStore.svelte.ts` (DB mirror + scan actions + album/artist grouping). Components: `TrackRow`, `LocalRow`, `AlbumCard`, `ShelfTile`, `CoverArt`, `ArtistSpotlight`.
- **`radio/`** — curated web radio. Data: `stations.ts`. Component: `StationCard`.
- **`settings/`** — playback preferences. Store: `PreferencesStore.svelte.ts`.

### The screens (`src/lib/screens/`)

Compositions that wire features together into user-facing experiences.
Atomic-design "pages" in the pure sense — they sit ABOVE the atomic
tiers and consume from them.

- **`NowPlayingScreen`** — hero art + metadata + waveform + transport + queue card + artist spotlight, fed by the player and library features.
- **`LibraryScreen`** — tabs over the library feature's groupings, with
  the design system's grid cards.
- **`DiscoverScreen`** — shelves derived from the library.
- **`RadioScreen`**, **`LocalFilesScreen`**, **`SpotifyScreen`**,
  **`SettingsScreen`** — one composition each; `AppTitlebar` is app
  chrome shared by the root layout.

- **Templates** — SvelteKit `+layout.svelte` files (e.g. the root layout
  with its `bootPromise` gate) wrap pages.
- **Routes** — `src/routes/<name>/+page.svelte` are 1-line wrappers
  importing the matching screen.

The mandatory project rules in `CLAUDE.md` enforce the atomic discipline:

- **Rule 9** — every clickable icon button needs a corresponding `IconButtonIdentifier` entry. The enum is the **atom registry**.
- **Rule 11** — screens are thin containers (i.e. proper pages, not god-components).
- **Rule 15** — reuse-first. Before writing new markup, search atoms and molecules. A pattern repeated three times inline is a missing atom.

The component catalogue lives in the Design System section of `CLAUDE.md`.

---

## 9. Checklist Before Adding a Component

- [ ] Does an atom or molecule already cover this? (Search first.)
- [ ] Which tier does this belong to? (Atom / Molecule / Organism.)
- [ ] Is the API generic enough that **a second, unrelated caller** could
      reuse it without modification?
- [ ] Does it own only state appropriate to its tier?
- [ ] Are all user-facing strings going through `t('key')` (Janis rule 10 — new keys land in both `en.json` and `fr.json`)?
- [ ] Are all colours, sizes, and durations using semantic tokens — no
      Tailwind magic numbers (Janis rule 3a)?
- [ ] If it's reusable, is there a `<Component>.figma.ts` mapping?
- [ ] Has `mcp__svelte__svelte-autofixer` been run and reported clean?

---

## 10. One-Page Summary

| Stage        | What it is                                                                    | What it owns                                                                   | Good test                               |
| ------------ | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | --------------------------------------- |
| **Atom**     | Irreducible primitive                                                         | Visual variants via props                                                      | Could ship in any app                   |
| **Molecule** | Smallest functional unit                                                      | One job, tiny local state                                                      | Does exactly one thing                  |
| **Organism** | Distinct UI section                                                           | Section state, maybe store reads                                               | Stands alone in a doc page              |
| **Template** | Layout skeleton                                                               | Snippet regions and spacing                                                    | Holds without real content              |
| **Page**     | The most concrete stage: template populated with real, representative content | Orchestration of stores + organisms; no new visual decisions at the file level | Surfaces edge cases the template missed |

**The single most useful idea.** Atomic Design is a vocabulary, not a
process. Use the vocabulary to **stay deliberate** about where things
belong, then ship. Move freely between scales. When real content breaks a
pattern, fix the pattern, not the page.

---

## References

- Brad Frost, _Atomic Design_, Chapter 2 —
  <https://atomicdesign.bradfrost.com/chapter-2/>
- Propelius Technologies, _Atomic Design in React: Best Practices_ —
  <https://propelius.tech/blogs/atomic-design-in-react-best-practices/>
  (the source's React-specific advice has been translated to Svelte 5
  idioms throughout this document)
- Janis-specific application — `CLAUDE.md` (rules 9, 11, 15, and the Design System section, which doubles as the component catalogue)
