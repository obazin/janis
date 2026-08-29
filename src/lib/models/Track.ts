// Wire types for the library IPC surface. Mirror the Rust structs in
// `src-tauri/src/library.rs` (serde camelCase).

export interface Track {
    id: number;
    folderId: number | null;
    path: string;
    title: string;
    artist: string | null;
    album: string | null;
    composer: string | null;
    durationSecs: number;
    format: string;
    sampleRate: number | null;
    bitDepth: number | null;
    channels: number | null;
    lossless: boolean;
    addedAt: number;
    /** Position within the album — tagged, or recovered from the filename. */
    trackNumber: number | null;
    trackTotal: number | null;
    discNumber: number | null;
    discTotal: number | null;
    year: number | null;
    genre: string | null;
    /** The album's own artist, which on a compilation is not the track's. */
    albumArtist: string | null;
}

export interface WatchedFolder {
    id: number;
    path: string;
    trackCount: number;
}

export interface ScanReport {
    added: number;
    updated: number;
    skipped: number;
    removed: number;
}

export interface CoverArt {
    mime: string;
    dataBase64: string;
}
