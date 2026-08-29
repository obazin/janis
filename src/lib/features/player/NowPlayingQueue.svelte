<script lang="ts">
    import { playerStore } from './PlayerStore.svelte';
    import { libraryStore } from '$lib/features/library/LibraryStore.svelte';
    import { eqOpen } from '$lib/stores/EqOverlayStore';
    import { fmtTime } from './format';
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import type { Track } from '$lib/models/Track';
    import type { TranslationKey } from '$lib/i18n/types';

    // Now Playing "what's next" card. Replaces the old SpectrumPanel with a
    // three-way toggle over the live play queue, the full queue as a
    // playlist, and the current album — keeping the Open-Equalizer
    // affordance in its header (the spectrum itself still lives in the
    // mini-player waveform + the EQ overlay).

    type View = 'queue' | 'playlist' | 'album';
    let view = $state<View>('queue');

    const queue = $derived(playerStore.queue);
    const current = $derived(playerStore.current);
    const currentIndex = $derived(current ? queue.findIndex((tr) => tr.id === current.id) : -1);

    // Album tracks for the current track, from the library grouping.
    const albumTracks = $derived.by<Track[]>(() => {
        if (!current) return [];
        const group = libraryStore.albums.find(
            (a) => a.album === current.album && a.artist === current.artist,
        );
        return group?.tracks ?? [current];
    });

    interface Row {
        track: Track;
        display: number;
        list: Track[];
        listIndex: number;
        isCurrent: boolean;
    }

    const rows = $derived.by<Row[]>(() => {
        if (view === 'album') {
            return albumTracks.map((track, i) => ({
                track,
                display: i + 1,
                list: albumTracks,
                listIndex: i,
                isCurrent: track.id === current?.id,
            }));
        }
        if (view === 'playlist') {
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
        view === 'album'
            ? (current?.album ?? t('common.unknownAlbum'))
            : view === 'playlist'
              ? t('now.queueHeading', { count: queue.length })
              : t('now.upNextHeading', { count: rows.length }),
    );

    const TABS: { id: View; key: TranslationKey }[] = [
        { id: 'queue', key: 'now.tabUpNext' },
        { id: 'playlist', key: 'now.tabPlaylist' },
        { id: 'album', key: 'now.tabAlbum' },
    ];

    function play(row: Row) {
        playerStore.playQueue(row.list, row.listIndex);
    }
</script>

<div class="bg-well-soft border border-border rounded-panel px-4.5 py-4">
    <div class="flex justify-between items-center gap-3 mb-3.5">
        <div class="flex gap-1 bg-well rounded-lg p-1">
            {#each TABS as tab (tab.id)}
                <button
                    class="cursor-pointer text-caption font-bold px-3.25 py-1.5 rounded-md transition-colors duration-fast select-none
                    {view === tab.id
                        ? 'bg-accent text-on-accent'
                        : 'text-text-soft hover:text-text-secondary'}"
                    onclick={() => (view = tab.id)}
                >
                    {t(tab.key)}
                </button>
            {/each}
        </div>
        <button
            class="flex items-center gap-1.75 flex-none cursor-pointer text-caption font-bold text-accent px-3 py-1.5 bg-accent/12 rounded-full border border-accent/30 transition-colors duration-fast hover:bg-accent/20"
            onclick={() => eqOpen.set(true)}
        >
            <svg
                class="size-3.5"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
            >
                <path d={ICONS.equalizer} />
            </svg>
            {t('now.eq')}
        </button>
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
                    <ArtTile
                        seed="{row.track.artist ?? ''}-{row.track.album ?? row.track.title}"
                        class="size-8.5 rounded-lg flex-none"
                    />
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
