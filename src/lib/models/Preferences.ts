// Wire type for `get_preferences` — mirrors the Rust struct in
// `src-tauri/src/persistence.rs` (serde camelCase).

export interface Preferences {
    volume: number;
    eqGains: number[];
    eqPreset: string;
    gapless: boolean;
    crossfade: boolean;
    normalize: boolean;
    exclusive: boolean;
    language: string;
}

/** The four playback switches — the closed enum `set_playback_option` accepts. */
export type PlaybackOption = 'gapless' | 'crossfade' | 'normalize' | 'exclusive';
