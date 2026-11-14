# Web-radio catalog builder

Regenerates `data/stations.json`, the static list the Radio screen ships. The catalog is baked at build time — the app makes no directory calls at runtime, it only opens the stream you click.

## Sources

The output merges two inputs:

1. **`curated.json`** — hand-picked flagship stations (SomaFM, the Radio France webradios, Radio Paradise, Radio Swiss, KEXP/KCRW/NTS, …). Kept verbatim, including the `nowPlaying` metadata-provider wiring that only these stations have. This is the source of truth for the curated core — edit it by hand.
2. **[Radio Browser](https://www.radio-browser.info)** — the community station database. The builder fetches the most-listened working stations per target country (`order=clickcount`, `hidebroken=true`), deduplicates them, and maps their free-text tags onto our genre keys.

## Run it

```bash
just stations
```

It fetches live from a Radio Browser mirror (with a fallback and a polite delay between requests), then rewrites `stations.json` and prints a country/genre breakdown. Re-run it to refresh the catalog as stations come and go.

The builder is a small Rust package of its own in this directory (`Cargo.toml`, `src/`), with its own lock file: it shares nothing with the application's dependencies and is not part of a release build. `just check` lints and tests it with the rest.

| Option | Effect |
|---|---|
| `just stations --output <file>` | Writes the catalog somewhere other than `data/stations.json` |
| `just stations --mirror <url>` | Asks one Radio Browser mirror, or a local copy of its answers, instead of the built-in ones |
| `just stations --curated <file>` | Starts from another seed than `curated.json` |
| `just stations --pause <milliseconds>` | Waits that long between two requests instead of 400 ms; `0` against a local mirror |

## Tuning

- **Countries and volume** — edit `COUNTRY_CAPS` in `src/main.rs`: the key is an ISO 3166-1 alpha-2 code, the value is how many fresh stations to keep for it. The curated seed adds on top.
- **Genre top-ups** — `GENRE_PULLS` in `src/main.rs` deepens genres the per-country lists leave thin. Each entry `{ tag, genre, cap }` pulls the most-clicked stations for an exact Radio Browser tag, keeps up to `cap` of them (from the `COUNTRY_CAPS` countries only), and labels them with `genre` outright. Add an entry to boost a genre.
- **Genre mapping** — `GENRE_RULES` in `src/classification.rs` is an ordered list of `(genre key, [tag keywords])` used for the per-country pulls. Each station scores one point per tag that matches a keyword; the highest score wins, ties break by list order; a station that matches nothing falls back to `radio.genre.variety`.

## When the mapping introduces a new key

Genres and countries are shown through the translation catalogs, so `cargo test` fails until the keys exist in **both** `translations/en.json` and `fr.json` (a test checks every station's genre and country has a label, another that both catalogs hold the same keys):

- a new `radio.genre.<x>` used by `GENRE_RULES`
- a new `radio.country.<cc>` for every country code in `COUNTRY_CAPS`

After running it, verify with `just check`.
