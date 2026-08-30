import { invoke } from '@tauri-apps/api/core';
import type { Preferences, PlaybackOption } from '$lib/models/Preferences';
import { audioEngine } from '$lib/features/player/audioEngine';

// Persisted playback and window-chrome settings. Playback switches also push
// into the engine; title-bar changes apply when Janis next opens.
class PreferencesStore {
    #options = $state<Record<PlaybackOption, boolean>>({
        gapless: true,
        crossfade: false,
        normalize: true,
        exclusive: false,
    });

    #hideTitleBar = $state(false);
    #frameless = $state(false);

    get options(): Readonly<Record<PlaybackOption, boolean>> {
        return this.#options;
    }

    get hideTitleBar(): boolean {
        return this.#hideTitleBar;
    }

    get frameless(): boolean {
        return this.#frameless;
    }

    /** Boot hydration from the preferences row. */
    init(prefs: Preferences) {
        this.#options = {
            gapless: prefs.gapless,
            crossfade: prefs.crossfade,
            normalize: prefs.normalize,
            exclusive: prefs.exclusive,
        };
        this.#hideTitleBar = prefs.hideTitleBar;
        this.#frameless = prefs.hideTitleBar;
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

    setHideTitleBar(hidden: boolean) {
        this.#hideTitleBar = hidden;
        invoke('set_title_bar_hidden', { hidden }).catch((err) =>
            console.error('set_title_bar_hidden failed:', err),
        );
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
