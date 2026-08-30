# Web-radio catalog builder

Regenerates `src/lib/features/radio/stations.json`, the static list the Radio screen ships. The catalog is baked at build time — the app makes no directory calls at runtime, it only opens the stream you click.

## Sources

The output merges two inputs:

1. **`curated.json`** — hand-picked flagship stations (SomaFM, the Radio France webradios, Radio Paradise, Radio Swiss, KEXP/KCRW/NTS, …). Kept verbatim, including the `nowPlaying` metadata-provider wiring that only these stations have. This is the source of truth for the curated core — edit it by hand.
2. **[Radio Browser](https://www.radio-browser.info)** — the community station database. The script fetches the most-listened working stations per target country (`order=clickcount`, `hidebroken=true`), deduplicates them, and maps their free-text tags onto our genre keys.

## Run it

```bash
node scripts/radio/build-stations.mjs
```

It fetches live from a Radio Browser mirror (with a fallback and a polite delay between countries), then rewrites `stations.json` and prints a country/genre breakdown. Re-run it to refresh the catalog as stations come and go.

## Tuning

- **Countries and volume** — edit `CAPS` at the top: the key is an ISO 3166-1 alpha-2 code, the value is how many fresh stations to keep for it. The curated seed adds on top.
- **Genre top-ups** — `GENRE_PULLS` deepens genres the per-country lists leave thin. Each entry `{ tag, genre, cap }` pulls the most-clicked stations for an exact Radio Browser tag, keeps up to `cap` of them (from the `CAPS` countries only), and labels them with `genre` outright. Add an entry to boost a genre.
- **Genre mapping** — `GENRE_RULES` is an ordered list of `[genreKey, [tag keywords]]` used for the per-country pulls. Each station scores one point per tag that matches a keyword; the highest score wins, ties break by list order; a station that matches nothing falls back to `radio.genre.variety`.

## When the mapping introduces a new key

Genres and countries are rendered through i18n, so the type-check fails until the keys exist in **both** `src/lib/i18n/en.json` and `fr.json` (the `TranslationKey` type is derived from `en.json`):

- a new `radio.genre.<x>` used by `GENRE_RULES`
- a new `radio.country.<cc>` for every country code in `CAPS`

After running the script, verify with `pnpm check`.
