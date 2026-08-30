import type { Station } from '$lib/models/Station';
import type { TranslationKey } from '$lib/i18n/types';
import stationsData from './stations.json';

// The curated station list ships as static JSON (`stations.json`) — a merge of
// hand-picked flagship streams (SomaFM, Radio France, Radio Paradise, …) and the
// most-listened stations per country from the Radio Browser community database,
// baked in at build time. No network call is made at runtime. User-added
// stations are a future feature.
//
// The JSON widens `genreKey`/`country` to `string`, so the cast re-applies the
// `Station` contract; a stray genre key would surface as a missing translation.
export const STATIONS = stationsData as unknown as Station[];

/** Genre-filter chips: "All" + every genre present in the list, in order. */
export const GENRE_FILTERS: TranslationKey[] = [
    'radio.genre.all',
    ...STATIONS.map((s) => s.genreKey).filter((key, i, all) => all.indexOf(key) === i),
];

/** Country-filter chips: every country present, most-populated first. */
export const COUNTRY_FILTERS: string[] = (() => {
    const counts = new Map<string, number>();
    for (const s of STATIONS) counts.set(s.country, (counts.get(s.country) ?? 0) + 1);
    return [...counts.keys()].sort((a, b) => (counts.get(b) ?? 0) - (counts.get(a) ?? 0));
})();
