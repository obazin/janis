import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import type { Track, CoverArt } from '$lib/models/Track';
import type { Station } from '$lib/models/Station';
import { audioGraph } from './audioGraph';

// The playback engine. Setter-method rune class (side effects everywhere:
// two audio elements, the Web Audio graph, IPC persistence of volume), so
// every write goes through a method.
//
// Two sources, two elements:
//  - local tracks play through `audioGraph.element` (EQ + analyser);
//  - radio streams play through a plain element — a cross-origin stream
//    without CORS headers would be silenced by the graph, so streams skip
//    it and the visualizers run synthetic.
class PlayerStore {
    #queue = $state<Track[]>([]);
    #index = $state(0);
    #playing = $state(false);
    #mode = $state<'local' | 'radio' | null>(null);
    #station = $state<Station | null>(null);
    #volume = $state(0.8);
    #shuffle = $state(false);
    #repeat = $state(false);
    #currentTime = $state(0);
    #duration = $state(0);
    #coverUrl = $state<string | null>(null);

    #wired = false;
    #streamEl: HTMLAudioElement | null = null;
    #coverEpoch = 0;
    #volumeTimer: ReturnType<typeof setTimeout> | null = null;

    get queue(): readonly Track[] {
        return this.#queue;
    }
    get playing() {
        return this.#playing;
    }
    get mode() {
        return this.#mode;
    }
    get station() {
        return this.#station;
    }
    get volume() {
        return this.#volume;
    }
    get shuffle() {
        return this.#shuffle;
    }
    get repeat() {
        return this.#repeat;
    }
    get currentTime() {
        return this.#currentTime;
    }
    get duration() {
        return this.#duration;
    }
    get coverUrl() {
        return this.#coverUrl;
    }

    /** The local track playing/paused, or null in radio/idle mode. */
    get current(): Track | null {
        return this.#mode === 'local' ? (this.#queue[this.#index] ?? null) : null;
    }

    get progress(): number {
        return this.#duration > 0 ? this.#currentTime / this.#duration : 0;
    }

    /** Fractional progress read straight off the element — smooth at 60 fps. */
    liveProgress(): number {
        if (this.#mode !== 'local') return 0;
        const el = audioGraph.element;
        return el.duration > 0 ? el.currentTime / el.duration : this.progress;
    }

    get analyser(): AnalyserNode | null {
        return this.#mode === 'local' ? audioGraph.analyser : null;
    }

    /** Boot hydration (volume comes from the preferences row). */
    initVolume(volume: number) {
        this.#volume = Math.min(1, Math.max(0, volume));
    }

    #wire() {
        if (this.#wired) return;
        this.#wired = true;
        const el = audioGraph.element;
        el.addEventListener('timeupdate', () => {
            this.#currentTime = el.currentTime;
        });
        el.addEventListener('durationchange', () => {
            if (Number.isFinite(el.duration) && el.duration > 0) this.#duration = el.duration;
        });
        el.addEventListener('ended', () => this.#onEnded());
        el.addEventListener('pause', () => {
            if (this.#mode === 'local') this.#playing = !el.paused && this.#playing;
        });
    }

    /** Replaces the queue and starts at `index`. */
    playQueue(tracks: Track[], index: number) {
        if (!tracks.length) return;
        this.#stopStream();
        this.#queue = [...tracks];
        this.#index = Math.min(Math.max(0, index), tracks.length - 1);
        this.#mode = 'local';
        this.#load();
    }

    playStation(station: Station) {
        this.#wire();
        audioGraph.element.pause();
        this.#mode = 'radio';
        this.#station = station;
        this.#coverUrl = null;
        this.#currentTime = 0;
        this.#duration = 0;
        if (!this.#streamEl) this.#streamEl = new Audio();
        this.#streamEl.src = station.url;
        this.#streamEl.volume = this.#volume;
        this.#streamEl.play().catch((err) => console.error('radio play failed:', err));
        this.#playing = true;
    }

    toggle() {
        if (this.#mode === 'local') {
            const el = audioGraph.element;
            audioGraph.resume();
            if (this.#playing) {
                el.pause();
                this.#playing = false;
            } else {
                el.play().catch((err) => console.error('play failed:', err));
                this.#playing = true;
            }
        } else if (this.#mode === 'radio' && this.#streamEl) {
            if (this.#playing) {
                this.#streamEl.pause();
                this.#playing = false;
            } else {
                this.#streamEl.play().catch((err) => console.error('radio play failed:', err));
                this.#playing = true;
            }
        }
    }

    next() {
        this.#step(1);
    }

    previous() {
        this.#step(-1);
    }

    seekTo(fraction: number) {
        if (this.#mode !== 'local') return;
        const el = audioGraph.element;
        if (!(el.duration > 0)) return;
        const f = Math.min(1, Math.max(0, fraction));
        el.currentTime = f * el.duration;
        this.#currentTime = el.currentTime;
    }

    setVolume(volume: number) {
        const v = Math.min(1, Math.max(0, volume));
        this.#volume = v;
        audioGraph.element.volume = v;
        if (this.#streamEl) this.#streamEl.volume = v;
        // Debounced persist — a drag emits dozens of values per second.
        if (this.#volumeTimer) clearTimeout(this.#volumeTimer);
        this.#volumeTimer = setTimeout(() => {
            invoke('set_volume', { volume: v }).catch((err) =>
                console.error('set_volume failed:', err),
            );
        }, 300);
    }

    toggleShuffle() {
        this.#shuffle = !this.#shuffle;
    }

    toggleRepeat() {
        this.#repeat = !this.#repeat;
    }

    #step(direction: 1 | -1) {
        if (this.#mode !== 'local' || !this.#queue.length) return;
        const len = this.#queue.length;
        if (this.#shuffle && len > 1 && direction === 1) {
            let pick = this.#index;
            while (pick === this.#index) pick = Math.floor(Math.random() * len);
            this.#index = pick;
        } else {
            this.#index = (this.#index + direction + len) % len;
        }
        this.#load();
    }

    #onEnded() {
        if (this.#mode !== 'local') return;
        const atEnd = this.#index === this.#queue.length - 1;
        if (atEnd && !this.#repeat) {
            this.#playing = false;
            return;
        }
        this.#step(1);
    }

    #load() {
        const track = this.#queue[this.#index];
        if (!track) return;
        this.#wire();
        audioGraph.ensure();
        audioGraph.resume();
        const el = audioGraph.element;
        el.src = convertFileSrc(track.path);
        el.volume = this.#volume;
        el.play().catch((err) => console.error('play failed:', err));
        this.#playing = true;
        this.#currentTime = 0;
        this.#duration = track.durationSecs;
        void this.#loadCover(track);
    }

    #stopStream() {
        if (this.#streamEl) {
            this.#streamEl.pause();
            this.#streamEl.removeAttribute('src');
        }
        this.#station = null;
    }

    async #loadCover(track: Track) {
        const epoch = ++this.#coverEpoch;
        this.#coverUrl = null;
        try {
            const cover = await invoke<CoverArt | null>('get_track_cover', { trackId: track.id });
            if (epoch !== this.#coverEpoch) return;
            this.#coverUrl = cover ? `data:${cover.mime};base64,${cover.dataBase64}` : null;
        } catch (err) {
            console.error('get_track_cover failed:', err);
        }
    }
}

export const playerStore = new PlayerStore();
