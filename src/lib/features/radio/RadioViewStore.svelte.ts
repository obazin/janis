import type { TranslationKey } from '$lib/i18n/types';

// Ephemeral view state for the Radio screen: the active genre and country
// filters. Both are multi-select — an empty list means "all". Not persisted or
// backed by IPC, but the screen component is destroyed and recreated on every
// navigation, so this still has to live outside it — screen-local `$state`
// would reset the filters every time you left and came back (CLAUDE.md rule 12).
// `countries` holds ISO 3166-1 alpha-2 codes.
class RadioViewStore {
    genres = $state<TranslationKey[]>([]);
    countries = $state<string[]>([]);
}

export const radioViewStore = new RadioViewStore();
