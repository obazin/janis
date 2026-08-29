import { invoke } from '@tauri-apps/api/core';
import { audioGraph, EQ_BAND_COUNT, EQ_GAIN_RANGE } from '$lib/features/player/audioGraph';
import { EQ_PRESETS, type EqPresetName } from './presets';

// EQ state. Setter-method rune class: every write applies the gains to the
// Web Audio filters and persists via IPC, so methods are the chokepoint.
class EqStore {
    #gains = $state<number[]>(Array(EQ_BAND_COUNT).fill(0));
    #preset = $state<string>('flat');

    #persistTimer: ReturnType<typeof setTimeout> | null = null;

    get gains(): readonly number[] {
        return this.#gains;
    }

    get preset(): string {
        return this.#preset;
    }

    /** Boot hydration from the preferences row. */
    init(gains: number[], preset: string) {
        this.#gains = gains.length === EQ_BAND_COUNT ? [...gains] : Array(EQ_BAND_COUNT).fill(0);
        this.#preset = preset;
        audioGraph.applyGains(this.#gains);
    }

    /** One band moved by hand — value in dB, preset becomes `custom`. */
    setBand(index: number, value: number) {
        const v = Math.round(Math.min(EQ_GAIN_RANGE, Math.max(-EQ_GAIN_RANGE, value)) * 2) / 2;
        this.#gains[index] = v;
        this.#preset = 'custom';
        audioGraph.applyGains(this.#gains);
        this.#persistDebounced();
    }

    setPreset(name: EqPresetName) {
        this.#gains = [...EQ_PRESETS[name]];
        this.#preset = name;
        audioGraph.applyGains(this.#gains);
        this.#persist();
    }

    reset() {
        this.setPreset('flat');
    }

    #persistDebounced() {
        if (this.#persistTimer) clearTimeout(this.#persistTimer);
        this.#persistTimer = setTimeout(() => this.#persist(), 400);
    }

    #persist() {
        invoke('set_eq', { gains: [...this.#gains], preset: this.#preset }).catch((err) =>
            console.error('set_eq failed:', err),
        );
    }
}

export const eqStore = new EqStore();
