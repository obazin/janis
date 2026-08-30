import type { Track } from '$lib/models/Track';

export const LIBRARY_TABS = ['playlists', 'artists', 'albums', 'songs'] as const;
export type LibraryTab = (typeof LIBRARY_TABS)[number];

export type LibrarySelection = {
    kind: 'album' | 'artist';
    key: string;
    label: string;
    tracks: Track[];
} | null;

// Ephemeral view state for the Library screen: the active tab, and the
// album/artist browsed instead of played. Neither is persisted or backed by
// IPC, but the screen component is destroyed and recreated on every
// navigation, so this still has to live outside it — screen-local `$state`
// would reset both the moment you left and came back (CLAUDE.md rule 12).
class LibraryViewStore {
    tab = $state<LibraryTab>('albums');
    selection = $state<LibrarySelection>(null);
}

export const libraryViewStore = new LibraryViewStore();
