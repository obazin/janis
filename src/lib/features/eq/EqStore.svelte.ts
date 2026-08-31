import { invoke } from '@tauri-apps/api/core';
import { notificationStore } from '$lib/stores/NotificationStore.svelte';
import { audioEngine } from '$lib/features/player/audioEngine';
import { EQ_BAND_COUNT, EQ_GAIN_RANGE } from './bands';
import { EQ_PRESETS, type EqPresetName } from './presets';

// EQ state. Setter-method rune class: every write pushes the gains to the
// engine's filters and persists via IPC, so methods are the chokepoint.
//
// Applying and persisting are separate calls on purpose: `audio_set_eq`
// writes straight into the realtime parameter block so a slider move is
// audible on the next buffer, while `set_eq` writes the database and is
// debounced, because a drag emits dozens of values a second.
class EqStore {
    #gains = $state<number[]>(Array(EQ_BAND_COUNT).fill(0));
    #preset = $state<string>('flat');
    #linearPhase = $state(false);
    // What the engine reports the FIR mode costs, in seconds — 0 while off.
    // Read from its echo rather than assumed, because the filter length is
    // fixed in taps and so the delay depends on the device's sample rate.
    #latencySecs = $state(0);

    #persistTimer: ReturnType<typeof setTimeout> | null = null;

    get gains(): readonly number[] {
        return this.#gains;
    }

    get preset(): string {
        return this.#preset;
    }

    get linearPhase(): boolean {
        return this.#linearPhase;
    }

    get latencySecs(): number {
        return this.#latencySecs;
    }

    /** Boot hydration from the preferences row. */
    init(gains: number[], preset: string, linearPhase: boolean) {
        this.#gains = gains.length === EQ_BAND_COUNT ? [...gains] : Array(EQ_BAND_COUNT).fill(0);
        this.#preset = preset;
        this.#linearPhase = linearPhase;
        void audioEngine.setEq(this.#gains);
        void audioEngine.setFirEq(linearPhase);
    }

    /** One band moved by hand — value in dB, preset becomes `custom`. */
    setBand(index: number, value: number) {
        const v = Math.round(Math.min(EQ_GAIN_RANGE, Math.max(-EQ_GAIN_RANGE, value)) * 2) / 2;
        this.#gains[index] = v;
        this.#preset = 'custom';
        void audioEngine.setEq(this.#gains);
        this.#persistDebounced();
    }

    setPreset(name: EqPresetName) {
        this.#gains = [...EQ_PRESETS[name]];
        this.#preset = name;
        void audioEngine.setEq(this.#gains);
        this.#persist();
    }

    reset() {
        this.setPreset('flat');
    }

    /** The quality mode: realtime biquad filters, or the linear-phase FIR. */
    setLinearPhase(enabled: boolean) {
        this.#linearPhase = enabled;
        void audioEngine.setFirEq(enabled);
        invoke('set_eq_linear_phase', { enabled }).catch((err) => {
            // The mode is already live in the engine; only the memory of it
            // failed, so say so rather than let the next launch silently
            // forget the choice.
            console.error('set_eq_linear_phase failed:', err);
            notificationStore.error('error.preferences', { dedupeKey: 'eq-linear-phase' });
        });
    }

    /**
     * The engine's echo of the mode and its exact latency, replayed on
     * subscribe too — so a reloaded webview recovers both from the engine
     * that never stopped rather than from the database alone.
     */
    applyEngineFirEq(enabled: boolean, latencySecs: number) {
        this.#linearPhase = enabled;
        this.#latencySecs = latencySecs;
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
