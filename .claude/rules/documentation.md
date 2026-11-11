---
paths:
  - "**/*.md"
---

# Documentation

- `README.md` and `docs/specs.md` describe the application as it is now. No history, no "was", "used to", "no longer". Every user-facing feature is in `docs/specs.md` before its task is done.
- `decisions-log.md` is where history goes. A new decision gets the next number `D<n>` in its section, with what was decided, why, and its status. When a decision changes, update the entry to the new choice and keep the old one as a lettered sub-entry (`D10a`) marked superseded.
- `CLAUDE.md` and `.claude/rules/` hold instructions for working on the code; keep `CLAUDE.md` under 200 lines and move path-specific guidance into a rule with `paths`.
- `ATOMIC_DESIGN.md` is the methodology; the tiers as Janis applies them, and the component catalogue, are in `.claude/rules/slint-ui.md`.
- One paragraph per line: never hard-wrap Markdown.
- Commands, paths and identifiers in backticks; tables for reference material (variables, recipes, modules).
