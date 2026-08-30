export type NowPlayingQueueView = 'queue' | 'playlist' | 'album';

// Ephemeral view state for the Now Playing queue card's three-way toggle.
// Not persisted or backed by IPC, but the screen component is destroyed and
// recreated on every navigation, so this still has to live outside it —
// screen-local `$state` would reset the toggle every time you left and came
// back (CLAUDE.md rule 12).
class NowPlayingViewStore {
    view = $state<NowPlayingQueueView>('queue');
}

export const nowPlayingViewStore = new NowPlayingViewStore();
