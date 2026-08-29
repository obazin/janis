import type en from './en.json';

// The key set is derived from `en.json` — adding a key there is what makes
// it exist; every other language file must then cover it (enforced by the
// `Translations` shape below).
export type TranslationKey = keyof typeof en;
export type Translations = Record<TranslationKey, string>;
