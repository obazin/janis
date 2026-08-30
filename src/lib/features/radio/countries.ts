import type { TranslationKey } from '$lib/i18n/types';

// Country codes carried by stations are ISO 3166-1 alpha-2. The label and flag
// both derive from the code — the flag from Unicode regional-indicator symbols,
// the label from a matching `radio.country.<code>` translation key.

/** Emoji flag for an ISO 3166-1 alpha-2 code, e.g. "US" → 🇺🇸. */
export function countryFlag(code: string): string {
    return code
        .toUpperCase()
        .replace(/[A-Z]/g, (c) => String.fromCodePoint(0x1f1e6 + c.charCodeAt(0) - 65));
}

/** Translation key for a country's name, e.g. "US" → `radio.country.us`. */
export function countryLabelKey(code: string): TranslationKey {
    return `radio.country.${code.toLowerCase()}` as TranslationKey;
}
