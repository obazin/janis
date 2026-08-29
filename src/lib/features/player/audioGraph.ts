// The Web Audio graph for local playback: one hidden <audio> element routed
// through ten peaking BiquadFilters (the EQ) into an AnalyserNode (the
// waveform/spectrum source) and out to the destination.
//
// Radio streams deliberately do NOT pass through this graph: a cross-origin
// stream without CORS headers is silenced by MediaElementSource, so the
// player keeps a second, plain element for streams (no EQ/analyser there —
// the visualizers fall back to their synthetic animation).

export const CENTER_FREQS = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000] as const;
export const FREQ_LABELS = [
    '32',
    '64',
    '125',
    '250',
    '500',
    '1k',
    '2k',
    '4k',
    '8k',
    '16k',
] as const;
export const EQ_BAND_COUNT = 10;
export const EQ_GAIN_RANGE = 12;

class AudioGraph {
    #element: HTMLAudioElement | null = null;
    #ctx: AudioContext | null = null;
    #filters: BiquadFilterNode[] = [];
    #analyser: AnalyserNode | null = null;
    #pendingGains: number[] | null = null;

    /** The graph-bound element. Created lazily — browser only. */
    get element(): HTMLAudioElement {
        if (!this.#element) {
            this.#element = new Audio();
            this.#element.preload = 'auto';
        }
        return this.#element;
    }

    get analyser(): AnalyserNode | null {
        return this.#analyser;
    }

    get sampleRate(): number | null {
        return this.#ctx?.sampleRate ?? null;
    }

    /**
     * Builds the graph once. Must be called from a user gesture (play click)
     * so the AudioContext is allowed to run.
     */
    ensure() {
        if (this.#ctx) return;
        this.#ctx = new AudioContext();
        const source = this.#ctx.createMediaElementSource(this.element);
        this.#filters = CENTER_FREQS.map((freq) => {
            const f = this.#ctx!.createBiquadFilter();
            f.type = 'peaking';
            f.frequency.value = freq;
            f.Q.value = 1.1;
            f.gain.value = 0;
            return f;
        });
        let node: AudioNode = source;
        for (const f of this.#filters) {
            node.connect(f);
            node = f;
        }
        this.#analyser = this.#ctx.createAnalyser();
        this.#analyser.fftSize = 2048;
        node.connect(this.#analyser);
        this.#analyser.connect(this.#ctx.destination);
        if (this.#pendingGains) {
            this.applyGains(this.#pendingGains);
            this.#pendingGains = null;
        }
    }

    resume() {
        if (this.#ctx && this.#ctx.state === 'suspended') {
            void this.#ctx.resume();
        }
    }

    /** Applies EQ gains (dB, one per band). Buffered until the graph exists. */
    applyGains(gains: number[]) {
        if (!this.#filters.length) {
            this.#pendingGains = [...gains];
            return;
        }
        this.#filters.forEach((f, i) => {
            f.gain.value = gains[i] ?? 0;
        });
    }
}

export const audioGraph = new AudioGraph();
