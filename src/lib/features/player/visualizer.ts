// Shared per-frame visual data for the waveform canvases.
//
// The engine pushes a 170-byte frame roughly sixty times a second: 160
// waveform points then ten band magnitudes, already windowed, FFT'd, folded
// and smoothed in Rust. In the byte encoding, 128 is silence in the trace and
// the band range spans 0–255. Every playing source feeds it — radio decodes
// through the same engine path as a local file, so a station gets a real
// trace, and a flat one means the frame channel is broken, not "expected".
//
// With no frame — nothing loaded, or paused — the canvases fall back to a
// synthetic animation whose energy eases toward zero, so they never care
// which source fed them.
//
// Every canvas runs its own rAF loop and calls `tick(now)`; the first call
// of a frame does the work, later calls are no-ops.

import { EQ_BAND_COUNT } from '$lib/features/eq/bands';

const WAVE_POINTS = 160;

export class Visualizer {
    wave = new Float32Array(WAVE_POINTS);
    mags = new Float32Array(EQ_BAND_COUNT);
    energy = 0.05;

    #t = 0;
    #lastNow = 0;
    #lastFrame = -1;

    constructor() {
        this.mags.fill(0.15);
    }

    tick(now: number, frame: Uint8Array | null, playing: boolean, eqGains: readonly number[]) {
        if (now === this.#lastFrame) return;
        this.#lastFrame = now;
        const dt = Math.min(0.05, (now - this.#lastNow) / 1000) || 0.016;
        this.#lastNow = now;
        this.#t += dt;

        const targetEnergy = playing ? 1 : 0.04;
        this.energy += (targetEnergy - this.energy) * Math.min(1, dt * 4);

        if (frame && playing) {
            for (let i = 0; i < WAVE_POINTS; i++) {
                this.wave[i] = (frame[i] - 128) / 128;
            }
            for (let i = 0; i < EQ_BAND_COUNT; i++) {
                const v = frame[WAVE_POINTS + i] / 255;
                // A little easing on top of the engine's own smoothing, so a
                // dropped frame reads as a pause rather than a jolt.
                this.mags[i] += (v - this.mags[i]) * 0.4;
            }
        } else {
            for (let i = 0; i < EQ_BAND_COUNT; i++) {
                const base =
                    0.28 +
                    0.72 *
                        Math.abs(Math.sin(this.#t * (0.8 + i * 0.33) + i * 1.3)) *
                        (0.7 + 0.3 * Math.sin(this.#t * 2.3 + i));
                const eqFactor = Math.max(
                    0.25,
                    Math.min(2.2, Math.pow(10, (eqGains[i] ?? 0) / 20)),
                );
                const target = Math.min(1, base * this.energy * eqFactor);
                this.mags[i] += (target - this.mags[i]) * 0.18;
            }
            for (let i = 0; i < WAVE_POINTS; i++) {
                const x = i / WAVE_POINTS;
                const v =
                    0.55 * Math.sin(x * Math.PI * 2 * 3 + this.#t * 4) +
                    0.28 * Math.sin(x * Math.PI * 2 * 7 + this.#t * 6.5) +
                    0.16 * Math.sin(x * Math.PI * 2 * 13 + this.#t * 9);
                this.wave[i] = v * this.energy;
            }
        }
    }
}

export const visualizer = new Visualizer();

/**
 * Canvas colors resolved from the app.css tokens — canvases can't use
 * Tailwind classes, so they read the custom properties instead of holding
 * raw color literals.
 */
export interface VisualPalette {
    accent: string;
    violet: string;
    teal: string;
    playhead: string;
    track: string;
}

let palette: VisualPalette | null = null;

export function visualPalette(): VisualPalette {
    if (!palette) {
        const style = getComputedStyle(document.documentElement);
        const read = (name: string, fallback: string) =>
            style.getPropertyValue(name).trim() || fallback;
        palette = {
            accent: read('--color-accent', 'oklch(0.667 0.251 355.4)'),
            violet: read('--color-violet', 'oklch(0.599 0.230 286.2)'),
            teal: read('--color-teal', 'oklch(0.818 0.139 186.7)'),
            playhead: read('--color-text', 'oklch(0.953 0.021 306.8)'),
            track: read('--color-rail', 'oklch(1 0 0 / 0.12)'),
        };
    }
    return palette;
}
