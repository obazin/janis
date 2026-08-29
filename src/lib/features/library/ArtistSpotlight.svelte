<script lang="ts">
    import { libraryStore } from './LibraryStore.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import CoverArt from './CoverArt.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import type { Track } from '$lib/models/Track';

    // Fills the space under the queue card on Now Playing: more tracks by the
    // current artist + that artist's albums. Both derive from the real
    // library grouping — no play-count column, because the DB does not track
    // plays yet (see HANDOFF.md; the design mock's "Most Played / 2.4k plays"
    // is intentionally not fabricated here).
    interface Props {
        artist: string | null | undefined;
    }
    let { artist }: Props = $props();

    const group = $derived(
        artist ? libraryStore.artists.find((a) => a.artist === artist) : undefined,
    );
    const moreTracks = $derived((group?.tracks ?? []).slice(0, 4));
    const albums = $derived(
        artist ? libraryStore.albums.filter((a) => a.artist === artist).slice(0, 4) : [],
    );

    function playTrack(track: Track) {
        const list = group?.tracks ?? [track];
        playerStore.playQueue(
            list,
            list.findIndex((tr) => tr.id === track.id),
        );
    }
</script>

{#if artist && (moreTracks.length > 1 || albums.length > 0)}
    <div class="grid grid-cols-2 gap-6 mt-1.5">
        {#if moreTracks.length}
            <div>
                <SectionLabel class="mb-2.5">{t('now.moreFrom', { artist })}</SectionLabel>
                {#each moreTracks as track, i (track.id)}
                    <button
                        class="flex gap-2.5 items-center px-2.5 py-2 rounded-card cursor-pointer w-full text-left transition-colors duration-fast hover:bg-accent/8"
                        onclick={() => playTrack(track)}
                    >
                        <div
                            class="w-5.5 font-display font-black text-heading-sm text-accent flex-none"
                        >
                            {i + 1}
                        </div>
                        <div class="flex-1 min-w-0">
                            <div class="font-semibold text-body truncate">{track.title}</div>
                            <div class="text-micro text-text-muted truncate">
                                {track.album ?? t('common.unknownAlbum')}
                            </div>
                        </div>
                        <svg
                            class="size-3.5 text-text-muted flex-none"
                            viewBox="0 0 24 24"
                            fill="currentColor"
                        >
                            <path d={ICONS.play} />
                        </svg>
                    </button>
                {/each}
            </div>
        {/if}
        {#if albums.length}
            <div>
                <SectionLabel class="mb-2.5">{t('now.albumsBy')}</SectionLabel>
                <div class="grid grid-cols-2 gap-3">
                    {#each albums as album (album.key)}
                        <button
                            class="cursor-pointer text-left"
                            onclick={() => playerStore.playQueue(album.tracks, 0)}
                        >
                            <CoverArt
                                trackId={album.tracks[0]?.id ?? null}
                                seed="{album.artist ?? ''}-{album.album ?? ''}"
                                class="aspect-square rounded-xl mb-1.5 shadow-tile"
                            />
                            <div class="font-semibold text-caption truncate">
                                {album.album ?? t('common.unknownAlbum')}
                            </div>
                            <div class="text-micro text-text-muted truncate">
                                {t('library.tracks', { count: album.tracks.length })}
                            </div>
                        </button>
                    {/each}
                </div>
            </div>
        {/if}
    </div>
{/if}
