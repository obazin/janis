import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { Track, WatchedFolder, ScanReport, CoverArt } from '$lib/models/Track';

const AUDIO_EXTENSIONS = ['mp3', 'flac', 'wav', 'm4a', 'aac', 'ogg', 'opus', 'aif', 'aiff'];

export interface AlbumGroup {
    key: string;
    album: string | null;
    artist: string | null;
    /** In album order: disc, then track, then title for untagged files. */
    tracks: Track[];
    newestAddedAt: number;
    year: number | null;
    genre: string | null;
    /** Combined runtime, for the album's detail line. */
    durationSecs: number;
    /** True once every track carries a position, so the order is trustworthy. */
    ordered: boolean;
}

/**
 * Album running order. Untagged tracks sort last rather than to the front,
 * where a missing number would otherwise read as track zero.
 */
export function compareInAlbum(a: Track, b: Track): number {
    const disc = (a.discNumber ?? 1) - (b.discNumber ?? 1);
    if (disc !== 0) return disc;
    const at = a.trackNumber ?? Number.MAX_SAFE_INTEGER;
    const bt = b.trackNumber ?? Number.MAX_SAFE_INTEGER;
    if (at !== bt) return at - bt;
    return a.title.localeCompare(b.title);
}

export interface ArtistGroup {
    artist: string;
    tracks: Track[];
}

// The track library, mirrored from `janis.db`. Setter-method rune class
// (every mutation is an IPC round-trip followed by a refetch — the DB is
// the source of truth, this store is its reactive mirror).
class LibraryStore {
    #tracks = $state<Track[]>([]);
    #folders = $state<WatchedFolder[]>([]);
    #scanning = $state(false);
    #lastReport = $state<ScanReport | null>(null);
    /** Embedded art, keyed by track. `null` means the file carries none. */
    #covers = $state<Map<number, string | null>>(new Map());
    #coverRequests = new Set<number>();

    get tracks(): readonly Track[] {
        return this.#tracks;
    }
    get folders(): readonly WatchedFolder[] {
        return this.#folders;
    }
    get scanning() {
        return this.#scanning;
    }
    get lastReport(): ScanReport | null {
        return this.#lastReport;
    }

    /**
     * Albums, newest addition first, each in its own running order.
     *
     * Grouped by album + album artist so a compilation stays one album
     * instead of splintering into one entry per featured artist.
     */
    readonly albums = $derived.by<AlbumGroup[]>(() => {
        const groups = new Map<string, AlbumGroup>();
        for (const track of this.#tracks) {
            const artist = track.albumArtist ?? track.artist;
            const key = `${track.album ?? ''}::${artist ?? ''}`;
            let group = groups.get(key);
            if (!group) {
                group = {
                    key,
                    album: track.album,
                    artist,
                    tracks: [],
                    newestAddedAt: 0,
                    year: null,
                    genre: null,
                    durationSecs: 0,
                    ordered: true,
                };
                groups.set(key, group);
            }
            group.tracks.push(track);
            group.newestAddedAt = Math.max(group.newestAddedAt, track.addedAt);
            group.durationSecs += track.durationSecs;
            group.year ??= track.year;
            group.genre ??= track.genre;
            if (track.trackNumber === null) group.ordered = false;
        }
        for (const group of groups.values()) {
            group.tracks.sort(compareInAlbum);
        }
        return [...groups.values()].sort((a, b) => b.newestAddedAt - a.newestAddedAt);
    });

    /** Artists with their tracks, most tracks first. */
    readonly artists = $derived.by<ArtistGroup[]>(() => {
        const groups = new Map<string, ArtistGroup>();
        for (const track of this.#tracks) {
            const artist = track.artist ?? '';
            if (!artist) continue;
            let group = groups.get(artist);
            if (!group) {
                group = { artist, tracks: [] };
                groups.set(artist, group);
            }
            group.tracks.push(track);
        }
        return [...groups.values()].sort((a, b) => b.tracks.length - a.tracks.length);
    });

    /**
     * Embedded cover art for a track, or `null` while it loads and when the
     * file has none.
     *
     * Fetched once per track and cached: art crosses IPC as base64, so a
     * library of any size cannot afford to re-request it on every render.
     * Callers ask only for art they are about to show — see `AlbumCard`,
     * which waits until the tile scrolls into view.
     */
    coverFor(trackId: number): string | null {
        const cached = this.#covers.get(trackId);
        if (cached !== undefined) return cached;
        if (!this.#coverRequests.has(trackId)) {
            this.#coverRequests.add(trackId);
            void invoke<CoverArt | null>('get_track_cover', { trackId })
                .then((cover) => {
                    this.#setCover(
                        trackId,
                        cover ? `data:${cover.mime};base64,${cover.dataBase64}` : null,
                    );
                })
                .catch((err) => {
                    console.error('get_track_cover failed:', err);
                    this.#setCover(trackId, null);
                });
        }
        return null;
    }

    #setCover(trackId: number, url: string | null) {
        // A new Map, not a mutation: `$state` tracks the reference.
        const next = new Map(this.#covers);
        next.set(trackId, url);
        this.#covers = next;
    }

    /** Boot hydration (called from `+layout.svelte`'s bootPromise). */
    async init() {
        await this.#refresh();
    }

    /** Folder picker → register + scan. */
    async addFolder() {
        const dir = await open({ directory: true });
        if (typeof dir !== 'string') return;
        await this.#scan(() => invoke<ScanReport>('add_watched_folder', { path: dir }));
    }

    /** File picker → ad-hoc import. */
    async addFiles() {
        const picked = await open({
            multiple: true,
            filters: [{ name: 'Audio', extensions: AUDIO_EXTENSIONS }],
        });
        const paths = Array.isArray(picked) ? picked : typeof picked === 'string' ? [picked] : [];
        if (!paths.length) return;
        await this.#scan(() => invoke<ScanReport>('import_files', { paths }));
    }

    /** OS drag-and-drop paths (folders are rejected backend-side per file). */
    async importPaths(paths: string[]) {
        const audio = paths.filter((p) =>
            AUDIO_EXTENSIONS.includes(p.split('.').pop()?.toLowerCase() ?? ''),
        );
        if (!audio.length) return;
        await this.#scan(() => invoke<ScanReport>('import_files', { paths: audio }));
    }

    async removeFolder(folderId: number) {
        await invoke('remove_watched_folder', { folderId });
        await this.#refresh();
    }

    async rescan() {
        await this.#scan(() => invoke<ScanReport>('rescan_library'));
    }

    async #scan(run: () => Promise<ScanReport>) {
        this.#scanning = true;
        try {
            this.#lastReport = await run();
            await this.#refresh();
        } catch (err) {
            console.error('library scan failed:', err);
        } finally {
            this.#scanning = false;
        }
    }

    async #refresh() {
        const [tracks, folders] = await Promise.all([
            invoke<Track[]>('list_tracks'),
            invoke<WatchedFolder[]>('list_watched_folders'),
        ]);
        this.#tracks = tracks;
        this.#folders = folders;
        this.#covers = new Map();
        this.#coverRequests.clear();
    }
}

export const libraryStore = new LibraryStore();
