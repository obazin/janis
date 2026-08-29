// The IPC client for the Rust audio engine.
//
// Decoding, the EQ, the analyser and the output device all live in Rust now;
// this module is the whole of the frontend's side of that boundary. It sends
// transport commands and receives two streams back:
//
//  - `EngineEvent`s — transport state, position, format, device, errors;
//  - visualiser frames — 170 raw bytes, sixty times a second.
//
// The frames deliberately do not travel as JSON. Tauri delivers a raw payload
// under 1 KB with a direct `webview.eval`; at or above that it parks the data
// and makes the page fetch it with an extra round trip, which is unaffordable
// at this rate.

import { invoke, Channel } from '@tauri-apps/api/core';
import type { NowPlayingSource } from '$lib/models/Station';

export interface AudioDevice {
    id: string;
    name: string;
    isDefault: boolean;
}

export type PlaybackMode = 'idle' | 'local' | 'radio';

export interface QueueItem {
    trackId: number;
    path: string;
    durationSecs: number;
    gainDb: number;
}

export type EngineEvent =
    | {
          event: 'state';
          data: {
              playing: boolean;
              mode: PlaybackMode;
              index: number;
              queueLen: number;
              shuffle: boolean;
              repeat: boolean;
              stationId: string | null;
          };
      }
    | { event: 'position'; data: { positionSecs: number; durationSecs: number } }
    | { event: 'trackChanged'; data: { index: number } }
    | {
          event: 'streamMetadata';
          data: {
              title: string | null;
              artist: string | null;
              album: string | null;
              cover: string | null;
          };
      }
    | { event: 'format'; data: { sampleRate: number; channels: number; codec: string } }
    | { event: 'device'; data: { name: string; sampleRate: number; channels: number } }
    | { event: 'error'; data: { message: string } };

/** Waveform points then band magnitudes — must match `analyser::FRAME_BYTES`. */
const WAVE_POINTS = 160;
const FRAME_BYTES = 170;

class AudioEngine {
    /** The most recent visualiser frame, or null before the first arrives. */
    #frame: Uint8Array | null = null;
    #subscribed = false;

    get frame(): Uint8Array | null {
        return this.#frame;
    }

    get wavePoints(): number {
        return WAVE_POINTS;
    }

    /**
     * Attaches both channels. Safe to call again — the engine replaces its
     * subscribers rather than accumulating them, which matters because a dev
     * hot reload leaves the previous page's channels pointing at a dead
     * webview.
     */
    async subscribe(onEvent: (event: EngineEvent) => void): Promise<void> {
        const events = new Channel<EngineEvent>();
        events.onmessage = onEvent;

        const frames = new Channel<ArrayBuffer>();
        frames.onmessage = (buffer) => {
            const bytes = new Uint8Array(buffer);
            if (bytes.length === FRAME_BYTES) this.#frame = bytes;
        };

        await invoke('audio_subscribe', { events, frames });
        this.#subscribed = true;
    }

    get subscribed(): boolean {
        return this.#subscribed;
    }

    loadQueue(tracks: QueueItem[], index: number) {
        return invoke('audio_load_queue', { tracks, index });
    }

    /**
     * Resolves once the station is connected and buffered, so the caller can
     * tell "connecting" from "playing".
     */
    playStream(stationId: string, url: string, nowPlaying?: NowPlayingSource) {
        return invoke('audio_play_stream', { stationId, url, nowPlaying: nowPlaying ?? null });
    }

    play() {
        return invoke('audio_play');
    }

    pause() {
        return invoke('audio_pause');
    }

    toggle() {
        return invoke('audio_toggle');
    }

    stop() {
        return invoke('audio_stop');
    }

    next() {
        return invoke('audio_next');
    }

    previous() {
        return invoke('audio_previous');
    }

    jumpTo(index: number) {
        return invoke('audio_jump_to', { index });
    }

    seek(positionSecs: number) {
        return invoke('audio_seek', { positionSecs });
    }

    setVolume(volume: number) {
        return invoke('audio_set_volume', { volume });
    }

    setEq(gains: number[]) {
        return invoke('audio_set_eq', { gains });
    }

    setShuffle(enabled: boolean) {
        return invoke('audio_set_shuffle', { enabled });
    }

    setRepeat(enabled: boolean) {
        return invoke('audio_set_repeat', { enabled });
    }

    devices() {
        return invoke<AudioDevice[]>('audio_devices');
    }

    setDevice(deviceId: string | null) {
        return invoke('audio_set_device', { deviceId });
    }
}

export const audioEngine = new AudioEngine();
