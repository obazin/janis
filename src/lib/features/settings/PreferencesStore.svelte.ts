import { invoke } from '@tauri-apps/api/core';
import type { Preferences, PlaybackOption } from '$lib/models/Preferences';

// The playback switches (gapless / crossfade / normalize / exclusive).
// Setter-method rune class: a toggle persists via IPC. The switches are
// stored preferences today; the engine grows into them release by release.
class PreferencesStore {
    #options = $state<Record<PlaybackOption, boolean>>({
        gapless: true,
        crossfade: false,
        normalize: true,
        exclusive: false,
    });

    get options(): Readonly<Record<PlaybackOption, boolean>> {
        return this.#options;
    }

    /** Boot hydration from the preferences row. */
    init(prefs: Preferences) {
        this.#options = {
            gapless: prefs.gapless,
            crossfade: prefs.crossfade,
            normalize: prefs.normalize,
            exclusive: prefs.exclusive,
        };
    }

    toggle(option: PlaybackOption) {
        const enabled = !this.#options[option];
        this.#options[option] = enabled;
        invoke('set_playback_option', { option, enabled }).catch((err) =>
            console.error('set_playback_option failed:', err),
        );
    }
}

export const preferencesStore = new PreferencesStore();
