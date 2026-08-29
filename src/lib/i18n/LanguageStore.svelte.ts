import type { TranslationKey, Translations } from './types';
import { invoke } from '@tauri-apps/api/core';
import en from './en.json';
import fr from './fr.json';

export const LANGUAGES = ['en', 'fr'] as const;
export type Language = (typeof LANGUAGES)[number];

// Cache read by the inline <script> in src/app.html before SvelteKit
// hydration to set the `lang` attribute synchronously. DB is the source of
// truth; this is a mirror.
const FOUC_CACHE_KEY = 'janis-language';
const DEFAULT_LANGUAGE: Language = 'en';

const TRANSLATION_MAP: Record<Language, Translations> = { en, fr };

function isLanguage(value: string | null | undefined): value is Language {
    return value != null && (LANGUAGES as readonly string[]).includes(value);
}

/**
 * Writes the FOUC mirror, tolerating a storage backend that refuses — the
 * mirror is a convenience, and an unguarded write inside the boot path could
 * blank the app.
 */
function cacheForFouc(language: Language) {
    try {
        localStorage.setItem(FOUC_CACHE_KEY, language);
    } catch {
        // Next launch pays one frame with the default `lang`. Nothing else breaks.
    }
}

class LanguageStore {
    // Private + read-only getter: a write also sets the `lang` attribute, the
    // FOUC mirror and the persisted preference, so `set` is the single
    // chokepoint that keeps them in step.
    #current = $state<Language>(DEFAULT_LANGUAGE);

    get current(): Language {
        return this.#current;
    }

    set(language: Language) {
        this.#current = language;
        document.documentElement.setAttribute('lang', language);
        cacheForFouc(language);
        invoke('set_language', { language }).catch((err) => {
            console.error('set_language failed:', err);
        });
    }

    /** Hydrate from the pre-fetched preferences row (called once at boot from `+layout.svelte`). */
    init(language: string) {
        const validated = isLanguage(language) ? language : DEFAULT_LANGUAGE;
        this.#current = validated;
        document.documentElement.setAttribute('lang', validated);
        cacheForFouc(validated);
    }
}

export const languageStore = new LanguageStore();

export function t(key: TranslationKey, params?: Record<string, string | number>): string {
    let value = TRANSLATION_MAP[languageStore.current][key] ?? TRANSLATION_MAP.en[key] ?? key;
    if (params) {
        for (const [k, v] of Object.entries(params)) {
            value = value.replaceAll(`{${k}}`, String(v));
        }
    }
    return value;
}
