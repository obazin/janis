import type { Station } from '$lib/models/Station';
import type { TranslationKey } from '$lib/i18n/types';

// The curated station list. All streams are publicly documented station
// endpoints; user-added stations are a future feature.
export const STATIONS: Station[] = [
    {
        id: 'soma-groove-salad',
        name: 'SomaFM Groove Salad',
        genreKey: 'radio.genre.ambient',
        kbps: 256,
        url: 'https://ice1.somafm.com/groovesalad-256-mp3',
        gradIndex: 1,
    },
    {
        id: 'fip-jazz',
        name: 'FIP Jazz',
        genreKey: 'radio.genre.jazz',
        kbps: 128,
        url: 'https://icecast.radiofrance.fr/fipjazz-midfi.mp3',
        gradIndex: 3,
    },
    {
        id: 'nightwave-plaza',
        name: 'Nightwave Plaza',
        genreKey: 'radio.genre.vaporwave',
        kbps: 128,
        url: 'https://radio.plaza.one/mp3',
        gradIndex: 0,
    },
    {
        id: 'radio-paradise',
        name: 'Radio Paradise',
        genreKey: 'radio.genre.eclectic',
        kbps: 320,
        url: 'https://stream.radioparadise.com/aac-320',
        gradIndex: 2,
    },
    {
        id: 'kexp',
        name: 'KEXP Seattle',
        genreKey: 'radio.genre.indie',
        kbps: 160,
        url: 'https://kexp.streamguys1.com/kexp160.aac',
        gradIndex: 4,
    },
    {
        id: 'kusc',
        name: 'Classical KUSC',
        genreKey: 'radio.genre.classical',
        kbps: 128,
        url: 'https://playerservices.streamtheworld.com/api/livestream-redirect/KUSCMP128.mp3',
        gradIndex: 5,
    },
];

/** Genre-filter chips: "All" + every genre present in the list, in order. */
export const GENRE_FILTERS: TranslationKey[] = [
    'radio.genre.all',
    ...STATIONS.map((s) => s.genreKey).filter((key, i, all) => all.indexOf(key) === i),
];
