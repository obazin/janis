import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { Track, WatchedFolder, ScanReport } from '$lib/models/Track';

const AUDIO_EXTENSIONS = ['mp3', 'flac', 'wav', 'm4a', 'aac', 'ogg', 'opus', 'aif', 'aiff'];

export interface AlbumGroup {
    key: string;
    album: string | null;
    artist: string | null;
    tracks: Track[];
    newestAddedAt: number;
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

    /** Albums, newest addition first. Grouped by album+artist. */
    readonly albums = $derived.by<AlbumGroup[]>(() => {
        const groups = new Map<string, AlbumGroup>();
        for (const track of this.#tracks) {
            const key = `${track.album ?? ''}::${track.artist ?? ''}`;
            let group = groups.get(key);
            if (!group) {
                group = {
                    key,
                    album: track.album,
                    artist: track.artist,
                    tracks: [],
                    newestAddedAt: 0,
                };
                groups.set(key, group);
            }
            group.tracks.push(track);
            group.newestAddedAt = Math.max(group.newestAddedAt, track.addedAt);
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
    }
}

export const libraryStore = new LibraryStore();
