import type { TranslationKey } from '$lib/i18n/types';

// A curated web-radio station. The list ships with the app
// (`features/radio/stations.ts`); user-added stations are a future feature.
export interface Station {
    id: string;
    name: string;
    /** i18n key for the genre label — also the genre-filter chip key. */
    genreKey: TranslationKey;
    kbps: number;
    url: string;
    /** Index into the shared art-gradient palette (`ArtTile`). */
    gradIndex: number;
}
