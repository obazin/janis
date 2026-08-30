import type { TranslationKey } from '$lib/i18n/types';

/**
 * Where a station publishes what it is currently playing.
 *
 * Only for operators who run a real endpoint. Everything else falls back to
 * the ICY metadata carried in the stream itself, which the engine parses.
 *
 * `key` identifies the station within that provider: a SomaFM channel slug,
 * a Radio France numeric station id, or a Radio Paradise channel number.
 */
export type NowPlayingSource = {
    provider: 'somafm' | 'radiofrance' | 'radioparadise';
    key: string;
};

// A curated web-radio station. The list ships with the app as static data
// (`features/radio/stations.json`); user-added stations are a future feature.
export interface Station {
    id: string;
    name: string;
    /** ISO 3166-1 alpha-2 country code (uppercase) — drives the country filter. */
    country: string;
    /** i18n key for the genre label — also the genre-filter chip key. */
    genreKey: TranslationKey;
    kbps: number;
    url: string;
    /** Index into the shared art-gradient palette (`ArtTile`). */
    gradIndex: number;
    /** Absent when the station only publishes ICY metadata, or none at all. */
    nowPlaying?: NowPlayingSource;
}
