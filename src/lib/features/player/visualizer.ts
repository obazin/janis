// Shared per-frame visual data for the waveform + spectrum canvases.
//
// When the analyser is live (local playback through the Web Audio graph) the
// wave is the real time-domain signal and the bars are real frequency bins.
// Without an analyser (radio streams, paused, nothing loaded) both fall back
// to a synthetic animation whose energy eases toward zero — the same
// behaviour in either case, so the canvases never care which source fed them.
//
// Every canvas runs its own rAF loop and calls `tick(now)`; the first call
// of a frame does the work, later calls are no-ops.

import { EQ_BAND_COUNT } from './audioGraph';

const WAVE_POINTS = 160;

export class Visualizer {
    wave = new Float32Array(WAVE_POINTS);
    mags = new Float32Array(EQ_BAND_COUNT);
    energy = 0.05;

    #t = 0;
    #lastNow = 0;
    #lastFrame = -1;
    #freqData: Uint8Array<ArrayBuffer> | null = null;
    #timeData: Uint8Array<ArrayBuffer> | null = null;

    constructor() {
        this.mags.fill(0.15);
    }

    tick(now: number, analyser: AnalyserNode | null, playing: boolean, eqGains: readonly number[]) {
        if (now === this.#lastFrame) return;
        this.#lastFrame = now;
        const dt = Math.min(0.05, (now - this.#lastNow) / 1000) || 0.016;
        this.#lastNow = now;
        this.#t += dt;

        const targetEnergy = playing ? 1 : 0.04;
        this.energy += (targetEnergy - this.energy) * Math.min(1, dt * 4);

        if (analyser && playing) {
            if (!this.#freqData || this.#freqData.length !== analyser.frequencyBinCount) {
                this.#freqData = new Uint8Array(analyser.frequencyBinCount);
                this.#timeData = new Uint8Array(analyser.fftSize);
            }
            analyser.getByteFrequencyData(this.#freqData);
            analyser.getByteTimeDomainData(this.#timeData!);
            const n = this.#freqData.length;
            for (let i = 0; i < EQ_BAND_COUNT; i++) {
                const lo = Math.floor(Math.pow(n, i / EQ_BAND_COUNT));
                const hi = Math.max(lo + 1, Math.floor(Math.pow(n, (i + 1) / EQ_BAND_COUNT)));
                let sum = 0;
                let count = 0;
                for (let k = lo; k < hi && k < n; k++) {
                    sum += this.#freqData[k];
                    count++;
                }
                const v = count ? sum / count / 255 : 0;
                this.mags[i] += (v - this.mags[i]) * 0.4;
            }
            const time = this.#timeData!;
            for (let i = 0; i < WAVE_POINTS; i++) {
                const idx = Math.floor((i / WAVE_POINTS) * time.length);
                this.wave[i] = (time[idx] - 128) / 128;
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
