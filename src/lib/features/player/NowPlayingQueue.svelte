<script lang="ts">
    import type { Snippet } from 'svelte';
    import { playerStore } from './PlayerStore.svelte';
    import { nowPlayingViewStore, type NowPlayingQueueView } from './NowPlayingViewStore.svelte';
    import { fmtTime } from './format';
    import OpenEqButton from '$lib/features/eq/OpenEqButton.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import type { Track } from '$lib/models/Track';
    import type { TranslationKey } from '$lib/i18n/types';

    // Now Playing "what's next" card: a three-way toggle over the live play
    // queue, the full queue as a playlist, and the current album — with the
    // Open-Equalizer affordance in its header.
    //
    // Album data and cover rendering come in from the screen: resolving the
    // album is library-domain work, and screens are the one layer allowed to
    // bridge player and library.
    //
    // The toggle itself lives in `nowPlayingViewStore`, not screen-local
    // `$state` — this component is destroyed and recreated on every
    // navigation, and the toggle is meant to survive that (CLAUDE.md rule 12).

    interface Props {
        /** The current track's album in running order, resolved by the
         *  screen from the library grouping. */
        albumTracks: Track[];
        /** Renders one row's cover art — the art pipeline is the library's. */
        cover: Snippet<[Track]>;
    }

    let { albumTracks, cover }: Props = $props();

    const queue = $derived(playerStore.queue);
    const current = $derived(playerStore.current);
    const currentIndex = $derived(current ? queue.findIndex((tr) => tr.id === current.id) : -1);

    interface Row {
        track: Track;
        display: number;
        list: Track[];
        listIndex: number;
        isCurrent: boolean;
    }

    const rows = $derived.by<Row[]>(() => {
        if (nowPlayingViewStore.view === 'album') {
            return albumTracks.map((track, i) => ({
                track,
                // The album's own numbering, so a partially-ripped album shows
                // the positions the sleeve does.
                display: track.trackNumber ?? i + 1,
                list: albumTracks,
                listIndex: i,
                isCurrent: track.id === current?.id,
            }));
        }
        if (nowPlayingViewStore.view === 'playlist') {
            return queue.map((track, i) => ({
                track,
                display: i + 1,
                list: [...queue],
                listIndex: i,
                isCurrent: i === currentIndex,
            }));
        }
        // Up next — everything after the current track, wrapping once so the
        // queue always reads as a ring.
        const ring = [
            ...queue.slice(currentIndex + 1),
            ...queue.slice(0, Math.max(0, currentIndex)),
        ];
        return ring.map((track, i) => ({
            track,
            display: i + 1,
            list: [...queue],
            listIndex: queue.findIndex((tr) => tr.id === track.id),
            isCurrent: false,
        }));
    });

    const heading = $derived(
        nowPlayingViewStore.view === 'album'
            ? (current?.album ?? t('common.unknownAlbum'))
            : nowPlayingViewStore.view === 'playlist'
              ? t('now.queueHeading', { count: queue.length })
              : t('now.upNextHeading', { count: rows.length }),
    );

    const TABS: { id: NowPlayingQueueView; key: TranslationKey }[] = [
        { id: 'queue', key: 'now.tabUpNext' },
        { id: 'playlist', key: 'now.tabPlaylist' },
        { id: 'album', key: 'now.tabAlbum' },
    ];

    function play(row: Row) {
        if (nowPlayingViewStore.view === 'album') {
            // The album card can list tracks that are not in the queue at
            // all; playing one replaces the queue with the album.
            playerStore.playQueue(row.list, row.listIndex);
            return;
        }
        // Both queue views show the live queue itself: jump within it rather
        // than reloading it — a reload re-serializes every entry over IPC and
        // reshuffles the order the listener was just reading.
        playerStore.jumpTo(row.listIndex);
    }
</script>

<div class="bg-well-soft border border-border rounded-panel px-4.5 py-4">
    <div class="flex justify-between items-center gap-3 mb-3.5">
        <div class="flex gap-1 bg-well rounded-lg p-1">
            {#each TABS as tab (tab.id)}
                <button
                    class="cursor-pointer text-caption font-bold px-3.25 py-1.5 rounded-md transition-colors duration-fast select-none
                    {nowPlayingViewStore.view === tab.id
                        ? 'bg-accent text-on-accent'
                        : 'text-text-soft hover:text-text-secondary'}"
                    onclick={() => (nowPlayingViewStore.view = tab.id)}
                >
                    {t(tab.key)}
                </button>
            {/each}
        </div>
        <OpenEqButton labelKey="now.eq" />
    </div>

    <SectionLabel class="mb-1.5">{heading}</SectionLabel>

    {#if rows.length === 0}
        <div class="py-6 text-center text-body text-text-muted">{t('now.queueEmpty')}</div>
    {:else}
        <div class="max-h-59 overflow-y-auto -mx-1.5 mscroll">
            {#each rows as row (row.track.id + '-' + row.display)}
                <button
                    class="flex gap-3 items-center px-3 py-2.25 rounded-card cursor-pointer w-full text-left transition-colors duration-fast hover:bg-accent/8
                    {row.isCurrent ? 'bg-accent/10' : ''}"
                    onclick={() => play(row)}
                >
                    <div class="w-6.5 flex justify-center flex-none">
                        {#if row.isCurrent}
                            <svg
                                class="size-3.5 text-accent"
                                viewBox="0 0 24 24"
                                fill="currentColor"
                            >
                                <path d={ICONS.play} />
                            </svg>
                        {:else}
                            <span class="font-mono text-caption text-text-faint">
                                {String(row.display).padStart(2, '0')}
                            </span>
                        {/if}
                    </div>
                    {@render cover(row.track)}
                    <div class="flex-1 min-w-0">
                        <div
                            class="font-semibold text-body truncate {row.isCurrent
                                ? 'text-accent'
                                : ''}"
                        >
                            {row.track.title}
                        </div>
                        <div class="text-caption text-text-muted truncate">
                            {row.track.artist ?? t('common.unknownArtist')}
                        </div>
                    </div>
                    <div class="font-mono text-caption text-text-muted flex-none">
                        {fmtTime(row.track.durationSecs)}
                    </div>
                </button>
            {/each}
        </div>
    {/if}
</div>
