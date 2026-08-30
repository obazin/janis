import type { TranslationKey } from '$lib/i18n/types';

// Ephemeral view state for the Radio screen: the active genre and country
// filters. Not persisted or backed by IPC, but the screen component is
// destroyed and recreated on every navigation, so this still has to live
// outside it — screen-local `$state` would reset the filters every time you
// left and came back (CLAUDE.md rule 12). `country` is `'all'` or an ISO
// 3166-1 alpha-2 code.
class RadioViewStore {
    genre = $state<TranslationKey>('radio.genre.all');
    country = $state<string>('all');
}

export const radioViewStore = new RadioViewStore();
