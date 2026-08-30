import { invoke } from '@tauri-apps/api/core';
import type { Preferences, PlaybackOption } from '$lib/models/Preferences';
import { audioEngine } from '$lib/features/player/audioEngine';

// The playback switches (gapless / crossfade / normalize / exclusive).
// Setter-method rune class: a toggle persists via IPC, and the ones the engine
// acts on are pushed to it as well. Gapless, crossfade and normalization are
// all live in the engine; exclusive output is still a stored preference only.
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
        this.#push('gapless', prefs.gapless);
        this.#push('crossfade', prefs.crossfade);
        this.#push('normalize', prefs.normalize);
    }

    toggle(option: PlaybackOption) {
        const enabled = !this.#options[option];
        this.#options[option] = enabled;
        invoke('set_playback_option', { option, enabled }).catch((err) =>
            console.error('set_playback_option failed:', err),
        );
        this.#push(option, enabled);
    }

    /** Tells the engine about a switch it acts on. */
    #push(option: PlaybackOption, enabled: boolean) {
        if (option === 'gapless') {
            void audioEngine.setGapless(enabled);
        }
        if (option === 'crossfade') {
            void audioEngine.setCrossfade(enabled);
        }
        if (option === 'normalize') {
            void audioEngine.setNormalize(enabled);
        }
    }
}

export const preferencesStore = new PreferencesStore();
