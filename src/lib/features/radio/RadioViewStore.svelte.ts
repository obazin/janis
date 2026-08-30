import type { TranslationKey } from '$lib/i18n/types';

// Ephemeral view state for the Radio screen: the active genre filter. Not
// persisted or backed by IPC, but the screen component is destroyed and
// recreated on every navigation, so this still has to live outside it —
// screen-local `$state` would reset the filter every time you left and came
// back (CLAUDE.md rule 12).
class RadioViewStore {
    genre = $state<TranslationKey>('radio.genre.all');
}

export const radioViewStore = new RadioViewStore();
