# Atomic Design — Concepts & Methodology

> A practitioner's reference for Atomic Design, distilled from Brad Frost's _Atomic Design_ (Chapter 2) and translated to **Slint** idioms. This document is the conceptual companion to Janis's living design system under `ui/` (generic, domain-agnostic primitives in `ui/atoms`, `ui/molecules` and `ui/organisms`) and the per-feature domain components under `ui/features/` (audio-player components built on top of the design system). Atomic tiers (atoms / molecules / organisms) apply equally to both layers.

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

In Slint those regions are the `@children` placeholder of a component, or — for the window itself — the elements `app.slint` lays out around the open screen.

| Trait           | Detail                                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------------------- |
| Examples        | TwoColumnNowPlayingTemplate, DashboardTemplate, ModalShellTemplate                                      |
| Content         | Placeholder / lorem / mocked data                                                                       |
| Concerns        | Layout, spacing, responsive behaviour **and** content-shape constraints (sizes, lengths, cardinalities) |
| Not its concern | Real data binding, business logic, final copy                                                           |

In Janis, **`ui/app.slint` is template-shaped**: it owns the scaffolding that screens slot into.

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

## 6. Best Practices (Slint Edition)

### 6.1 Folder structure

Mirror the hierarchy on disk. **Janis separates the interface into three layers**: the design system (primitives), features (domain capabilities), and screens (compositions). Atomic tiers are a classification scheme inside the design system and inside each feature folder.

```
ui/
├── tokens.slint            every color, typeface, text size, radius, shadow, duration
├── icons.slint             glyph path data
├── shell.slint             the window frame's state
├── app.slint               the window: the only file that assembles screens
├── atoms/                  design system, tier 1
├── molecules/              design system, tier 2
├── organisms/              design system, tier 3
├── features/
│   ├── player/             a feature's components, and the global they read
│   ├── equalizer/
│   ├── library/
│   ├── radio/
│   └── settings/
└── screens/                one composition per destination
```

Dependencies run one way: the design system imports nothing but tokens and icons; features import the design system (and the feature edges `CLAUDE.md` allows); screens import anything.

### 6.2 Naming conventions

One component per file. The file is the component's name in snake case (`art_tile.slint` holds `ArtTile`). Names say what the thing is, in full words, never how it looks today. A component private to a file — a row of one list, a tab of one card — stays unexported in that file.

### 6.3 Properties and component interfaces

- Declare every input as an `in property` with a type, and every event as a `callback`; a component never reaches into its parent.
- Offer variants as an `enum` property (`BadgeVariant`, `PlayCircleVariant`), not as a pile of booleans.
- Take text as properties. A design-system component never names a translation key; features and screens pass it `Strings.<key>`.
- An icon-only button takes its accessible label and its `IconButtonIdentifier`.

### 6.4 State ownership

- **Design-system components** own only what is theirs to show: hover, press, an animation phase.
- **Feature components** read their feature's global (`Player`, `Equalizer`, `Library`, `Radio`, `Settings`) and call its callbacks. The globals are filled by the application controller; a component never computes what Rust already did.
- **Screens** hold layout state only — which rows are in view, where a section sits. Anything the listener should find again after leaving the screen lives in the controller, because a screen is created anew each time it is opened.

### 6.5 Bindings

Slint properties are reactive: a binding is re-evaluated when what it reads changes. Prefer a binding to a `changed` handler; keep handlers for effects (scrolling something into view, reporting a size to a global). Two things worth knowing:

- Element-level `opacity` on something that animates forces an offscreen layer; carry transparency in the color instead.
- A layout cannot depend on a size it determines — bind to the parent's width, never to a value derived from the child's own preferred size.

### 6.6 Long lists

A list of fixed-height rows draws only the rows in view. The screen's `ScrollArea` says how far it is scrolled; the list computes which rows that shows and places each at `index × row height`. A row per item is never instantiated.

### 6.7 Validation against design

Look at the result. Debug builds save what the window shows (`JANIS_SNAPSHOT=<file.png>`, with `JANIS_SCREEN`, `JANIS_LIBRARY`, `JANIS_SEARCH`, `JANIS_EQUALIZER`, `JANIS_AUTOPLAY` and `JANIS_STATION` to reach a state), which is how a change to the interface is checked.

---

## 7. Worked Examples

### 7.1 Decomposing an e-commerce header

| Tier      | Element                                                                                             |
| --------- | --------------------------------------------------------------------------------------------------- |
| Atoms     | Logo, Input, IconButton (search), NavLink, CartIcon, Badge (count)                                  |
| Molecules | SearchField (Input + IconButton), NavItem (NavLink + optional Badge), CartButton (CartIcon + Badge) |
| Organism  | Header (Logo + Nav (× NavItem) + SearchField + CartButton + UserMenu)                               |
| Template  | StoreLayout — regions for header, body, footer                                                      |
| Page      | `/products/[slug]` — real product data                                                              |

Update one atom (say, `IconButton`) and **every consumer benefits**. That
is the practical payoff.

### 7.2 Decomposing a dashboard

| Tier      | Element                                                                                                  |
| --------- | -------------------------------------------------------------------------------------------------------- |
| Atoms     | Label, Number, Icon, Button, ToggleSwitch                                                                |
| Molecules | MetricCard (Label + Number), FilterDropdown (Button + menu), ToolbarButton                               |
| Organisms | Sidebar (Nav + UserChip), ChartPanel (FilterDropdown + Chart + export Button), MetricGrid (× MetricCard) |
| Template  | DashboardLayout — sidebar + main + toolbar regions                                                       |
| Page      | `/dashboard` — real metrics, real user, real filters                                                     |

The same `DashboardLayout` template can host analytics, user management, or
sales pages just by swapping the organisms rendered into each region.

---

## 8. How Janis Applies This

Janis separates **the design system** (generic, domain-agnostic primitives in `ui/atoms`, `ui/molecules` and `ui/organisms`) from **the application** (audio-player components under `ui/features/`), with **screens** (`ui/screens/`) composing features into what the listener sees and `ui/app.slint` as the template they slot into. Atomic-design tiering classifies components inside both layers.

The tiers as they stand — every component, the allowed dependencies between layers, how data flows to and from Rust, and the Slint specifics — are in [`.claude/rules/slint-ui.md`](.claude/rules/slint-ui.md), which is the reference to keep current when a component is added. The mandatory rules in `CLAUDE.md` enforce the discipline: atomic tiers, tokens only, reuse first, and search → ask → create for new components.

---

## 9. Checklist Before Adding a Component

- [ ] Does an atom or molecule already cover this? (Search first.)
- [ ] Which tier does this belong to? (Atom / Molecule / Organism.)
- [ ] Is the API generic enough that **a second, unrelated caller** could
      reuse it without modification?
- [ ] Does it own only state appropriate to its tier?
- [ ] Does all user-facing text come from the `Strings` global or from the controller (new keys land in both `translations/en.json` and `fr.json`)?
- [ ] Do all colours, type sizes, radii, shadows and durations come from `ui/tokens.slint`?
- [ ] Has it been looked at in the running window (a snapshot), not only compiled?

---

## 10. One-Page Summary

| Stage        | What it is                                                                    | What it owns                                                                   | Good test                               |
| ------------ | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | --------------------------------------- |
| **Atom**     | Irreducible primitive                                                         | Visual variants via props                                                      | Could ship in any app                   |
| **Molecule** | Smallest functional unit                                                      | One job, tiny local state                                                      | Does exactly one thing                  |
| **Organism** | Distinct UI section                                                           | Section state, maybe store reads                                               | Stands alone in a doc page              |
| **Template** | Layout skeleton                                                               | Regions and spacing                                                            | Holds without real content              |
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
  (the source's React-specific advice has been translated to Slint idioms throughout this document)
- Janis-specific application — `CLAUDE.md` (the mandatory rules) and `.claude/rules/slint-ui.md` (the tiers and the component catalogue)
