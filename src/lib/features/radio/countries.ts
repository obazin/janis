import type { TranslationKey } from '$lib/i18n/types';

// Country codes carried by stations are ISO 3166-1 alpha-2. The label derives
// from the code via a matching `radio.country.<code>` translation key.

/** Translation key for a country's name, e.g. "US" → `radio.country.us`. */
export function countryLabelKey(code: string): TranslationKey {
    return `radio.country.${code.toLowerCase()}` as TranslationKey;
}
