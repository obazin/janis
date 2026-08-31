import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Preferences, PlaybackOption } from '$lib/models/Preferences';
import { audioEngine } from '$lib/features/player/audioEngine';
import { isMac } from '$lib/utils/platform';

// The playback switches (gapless / crossfade / normalize / exclusive) plus the
// frameless-window toggle. Setter-method rune class: a toggle persists via IPC,
// and the ones the engine acts on are pushed to it as well. Gapless, crossfade
// and normalization are all live in the engine; exclusive output is still a
// stored preference only.
//
// Hiding the title bar drops the window's native decorations at runtime via
// `setDecorations`, so the toggle takes effect instantly. macOS keeps its
// overlay title bar untouched — the decorations there are the traffic lights.
class PreferencesStore {
    #options = $state<Record<PlaybackOption, boolean>>({
        gapless: true,
        crossfade: false,
        normalize: true,
        exclusive: false,
    });
    #hideTitleBar = $state(false);

    get options(): Readonly<Record<PlaybackOption, boolean>> {
        return this.#options;
    }

    /** True when the native window frame is hidden and the app draws its own. */
    get frameless(): boolean {
        return this.#hideTitleBar;
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
        this.#hideTitleBar = prefs.hideTitleBar;
        void this.#applyDecorations();
    }

    setTitleBarHidden(hidden: boolean) {
        this.#hideTitleBar = hidden;
        invoke('set_title_bar_hidden', { hidden }).catch((err) =>
            console.error('set_title_bar_hidden failed:', err),
        );
        void this.#applyDecorations();
    }

    /** Reflects the frameless preference onto the live window (non-macOS). */
    async #applyDecorations() {
        if (isMac()) return;
        try {
            await getCurrentWindow().setDecorations(!this.#hideTitleBar);
        } catch (err) {
            console.error('setDecorations failed:', err);
        }
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
