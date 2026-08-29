import { invoke } from '@tauri-apps/api/core';
import type { Track, CoverArt } from '$lib/models/Track';
import type { Station } from '$lib/models/Station';
import { audioEngine, type EngineEvent } from './audioEngine';

// The playback store. Rust owns the queue, the transport and the signal path;
// this is a reactive mirror of the engine plus the bits of presentation the
// engine has no business knowing about (the `Track` objects the UI renders,
// and cover art).
//
// Setter-method rune class: every write either sends an IPC command or
// persists something, so nothing is exposed as a public `$state` field.
//
// Radio goes through the same engine as local files, so a station gets the
// equalizer and a real visualiser like anything else. The store keeps the
// `Station` object for rendering; the engine only knows its id.
class PlayerStore {
    #queue = $state<Track[]>([]);
    #index = $state(0);
    #playing = $state(false);
    #engineMode = $state<'idle' | 'local' | 'radio'>('idle');
    #streamTitle = $state<string | null>(null);
    #streamArtist = $state<string | null>(null);
    #streamAlbum = $state<string | null>(null);
    #streamCover = $state<string | null>(null);
    #connecting = $state(false);
    #station = $state<Station | null>(null);
    #volume = $state(0.8);
    #shuffle = $state(false);
    #repeat = $state(false);
    #currentTime = $state(0);
    #duration = $state(0);
    #coverUrl = $state<string | null>(null);
    #sampleRate = $state<number | null>(null);
    #deviceName = $state<string | null>(null);

    #coverEpoch = 0;
    #volumeTimer: ReturnType<typeof setTimeout> | null = null;
    /** Wall-clock reading of the last `position` event, for interpolation. */
    #positionAt = 0;

    get queue(): readonly Track[] {
        return this.#queue;
    }
    get playing() {
        return this.#playing;
    }
    get mode(): 'local' | 'radio' | null {
        if (this.#engineMode === 'idle') return null;
        return this.#engineMode;
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
    /** Art for whatever is playing: the track's own, or the station's. */
    get coverUrl() {
        return this.mode === 'radio' ? this.#streamCover : this.#coverUrl;
    }
    /** What the station says it is playing, when it says anything. */
    get streamTitle() {
        return this.#streamTitle;
    }
    get streamArtist() {
        return this.#streamArtist;
    }
    get streamAlbum() {
        return this.#streamAlbum;
    }
    /** True while a station is connecting and buffering. */
    get connecting() {
        return this.#connecting;
    }
    /** Device sample rate, for the Settings readout. */
    get sampleRate() {
        return this.#sampleRate;
    }
    get deviceName() {
        return this.#deviceName;
    }

    /** The local track playing/paused, or null in radio/idle mode. */
    get current(): Track | null {
        return this.mode === 'local' ? (this.#queue[this.#index] ?? null) : null;
    }

    get progress(): number {
        return this.#duration > 0 ? this.#currentTime / this.#duration : 0;
    }

    /**
     * Fractional progress, interpolated between the engine's ~10 Hz position
     * events so the playhead stays smooth at 60 fps without paying for 60 Hz
     * of IPC.
     */
    liveProgress(): number {
        if (this.mode !== 'local' || this.#duration <= 0) return 0;
        const elapsed = this.#playing ? (performance.now() - this.#positionAt) / 1000 : 0;
        const at = Math.min(this.#duration, this.#currentTime + elapsed);
        return at / this.#duration;
    }

    /**
     * The latest visualiser frame from the engine, or null when idle. Radio
     * has one too now — it decodes through the same path as a local file.
     */
    get visualFrame(): Uint8Array | null {
        return this.mode === null ? null : audioEngine.frame;
    }

    /** Boot hydration (volume comes from the preferences row). */
    initVolume(volume: number) {
        const v = Math.min(1, Math.max(0, volume));
        this.#volume = v;
        void audioEngine.setVolume(v);
    }

    /** Attaches to the engine's event stream. Called once, from boot. */
    async connect() {
        await audioEngine.subscribe((event) => this.#apply(event));
    }

    #apply(event: EngineEvent) {
        switch (event.event) {
            case 'state':
                this.#engineMode = event.data.mode;
                this.#index = event.data.index;
                this.#shuffle = event.data.shuffle;
                this.#repeat = event.data.repeat;
                this.#playing = event.data.playing;
                if (event.data.mode !== 'radio') {
                    this.#station = null;
                    this.#clearStreamInfo();
                }
                break;
            case 'position':
                this.#currentTime = event.data.positionSecs;
                this.#duration = event.data.durationSecs;
                this.#positionAt = performance.now();
                break;
            case 'streamMetadata':
                this.#streamTitle = event.data.title;
                this.#streamArtist = event.data.artist;
                this.#streamAlbum = event.data.album;
                this.#streamCover = event.data.cover;
                break;
            case 'trackChanged':
                this.#index = event.data.index;
                void this.#loadCover(this.#queue[event.data.index]);
                break;
            case 'device':
                this.#deviceName = event.data.name;
                this.#sampleRate = event.data.sampleRate;
                break;
            case 'format':
                break;
            case 'error':
                console.error('audio engine:', event.data.message);
                break;
        }
    }

    /** Replaces the queue and starts at `index`. */
    playQueue(tracks: Track[], index: number) {
        if (!tracks.length) return;
        this.#station = null;
        this.#clearStreamInfo();
        this.#queue = [...tracks];
        this.#index = Math.min(Math.max(0, index), tracks.length - 1);
        this.#engineMode = 'local';
        this.#playing = true;
        this.#currentTime = 0;
        this.#duration = tracks[this.#index]?.durationSecs ?? 0;
        this.#positionAt = performance.now();
        void this.#loadCover(this.#queue[this.#index]);
        void audioEngine.loadQueue(
            tracks.map((track) => ({
                trackId: track.id,
                path: track.path,
                durationSecs: track.durationSecs,
                gainDb: 0,
            })),
            this.#index,
        );
    }

    playStation(station: Station) {
        this.#station = station;
        this.#clearStreamInfo();
        this.#coverUrl = null;
        this.#currentTime = 0;
        this.#duration = 0;
        this.#connecting = true;
        // Resolves only once the station is connected and buffered, which is
        // why the card can show a connecting state rather than pretending.
        audioEngine
            .playStream(station.id, station.url)
            .catch((err) => {
                console.error('radio play failed:', err);
                this.#station = null;
            })
            .finally(() => {
                this.#connecting = false;
            });
    }

    toggle() {
        if (this.mode === null) return;
        this.#playing = !this.#playing;
        this.#positionAt = performance.now();
        void audioEngine.toggle();
    }

    next() {
        void audioEngine.next();
    }

    previous() {
        void audioEngine.previous();
    }

    /** Plays a specific position in the current queue. */
    jumpTo(index: number) {
        void audioEngine.jumpTo(index);
    }

    seekTo(fraction: number) {
        if (this.mode !== 'local' || !(this.#duration > 0)) return;
        const f = Math.min(1, Math.max(0, fraction));
        this.#currentTime = f * this.#duration;
        this.#positionAt = performance.now();
        void audioEngine.seek(this.#currentTime);
    }

    setVolume(volume: number) {
        const v = Math.min(1, Math.max(0, volume));
        this.#volume = v;
        void audioEngine.setVolume(v);
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
        void audioEngine.setShuffle(this.#shuffle);
    }

    toggleRepeat() {
        this.#repeat = !this.#repeat;
        void audioEngine.setRepeat(this.#repeat);
    }

    #clearStreamInfo() {
        this.#streamTitle = null;
        this.#streamArtist = null;
        this.#streamAlbum = null;
        this.#streamCover = null;
    }

    async #loadCover(track: Track | undefined) {
        const epoch = ++this.#coverEpoch;
        this.#coverUrl = null;
        if (!track) return;
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
