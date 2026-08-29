<script lang="ts">
    import { libraryStore } from '$lib/features/library/LibraryStore.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import { searchQuery } from '$lib/stores/SearchStore';
    import type { Track } from '$lib/models/Track';
    import AlbumCard from '$lib/features/library/AlbumCard.svelte';
    import TrackRow from '$lib/features/library/TrackRow.svelte';
    import Chip from '$lib/design-system/atoms/Chip.svelte';
    import Eyebrow from '$lib/design-system/atoms/Eyebrow.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import PrimaryButton from '$lib/design-system/atoms/PrimaryButton.svelte';
    import EmptyState from '$lib/design-system/atoms/EmptyState.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import type { TranslationKey } from '$lib/i18n/types';

    const TABS = ['playlists', 'artists', 'albums', 'songs'] as const;
    type Tab = (typeof TABS)[number];
    const TAB_KEYS: Record<Tab, TranslationKey> = {
        playlists: 'library.tab.playlists',
        artists: 'library.tab.artists',
        albums: 'library.tab.albums',
        songs: 'library.tab.songs',
    };

    let tab = $state<Tab>('albums');

    function matches(track: Track, query: string): boolean {
        return [track.title, track.artist, track.album, track.composer]
            .filter((v): v is string => v != null)
            .some((v) => v.toLowerCase().includes(query));
    }

    const query = $derived($searchQuery.trim().toLowerCase());
    const filteredTracks = $derived(
        query ? libraryStore.tracks.filter((tr) => matches(tr, query)) : [...libraryStore.tracks],
    );
    const filteredAlbums = $derived(
        libraryStore.albums.filter((a) => a.tracks.some((tr) => filteredTracks.includes(tr))),
    );
    const filteredArtists = $derived(
        libraryStore.artists.filter((a) => a.tracks.some((tr) => filteredTracks.includes(tr))),
    );
    const recentTracks = $derived(filteredTracks.slice(0, 8));

    function playFrom(list: readonly Track[], track: Track) {
        playerStore.playQueue([...list], list.indexOf(track));
    }
</script>

<div class="px-11 pt-9 pb-10 animate-float-up">
    <div class="flex items-end justify-between mb-6">
        <div>
            <Eyebrow color="violet" class="mb-2">{t('library.eyebrow')}</Eyebrow>
            <h1 class="font-display font-black text-display tracking-tight m-0">
                {t('library.title')}
            </h1>
        </div>
        <PrimaryButton icon={ICONS.plus} onclick={() => libraryStore.addFiles()}>
            {t('library.addMusic')}
        </PrimaryButton>
    </div>

    {#if libraryStore.tracks.length === 0}
        <EmptyState
            icon={ICONS.library}
            title={t('library.empty.title')}
            description={t('library.empty.desc')}
            cta={{
                label: t('local.addFolder'),
                icon: ICONS.plus,
                onclick: () => libraryStore.addFolder(),
            }}
        />
    {:else}
        <div class="flex gap-2 mb-6">
            {#each TABS as candidate (candidate)}
                <Chip
                    label={t(TAB_KEYS[candidate])}
                    active={tab === candidate}
                    onclick={() => (tab = candidate)}
                />
            {/each}
        </div>

        {#if tab === 'playlists'}
            <EmptyState
                icon={ICONS.library}
                title={t('library.playlistsSoon.title')}
                description={t('library.playlistsSoon.desc')}
            />
        {:else if tab === 'albums'}
            <div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-5 mb-9">
                {#each filteredAlbums as album (album.key)}
                    <AlbumCard
                        title={album.album ?? t('common.unknownAlbum')}
                        subtitle={album.artist ?? t('common.unknownArtist')}
                        seed="{album.artist ?? ''}-{album.album ?? ''}"
                        onclick={() => playerStore.playQueue(album.tracks, 0)}
                    />
                {/each}
            </div>
        {:else if tab === 'artists'}
            <div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-5 mb-9">
                {#each filteredArtists as artist (artist.artist)}
                    <AlbumCard
                        title={artist.artist}
                        subtitle={t('library.tracks', { count: artist.tracks.length })}
                        seed={artist.artist}
                        onclick={() => playerStore.playQueue(artist.tracks, 0)}
                    />
                {/each}
            </div>
        {:else}
            <div class="rounded-card overflow-hidden border border-border mb-9">
                {#each filteredTracks as track, i (track.id)}
                    <TrackRow {track} index={i} onclick={() => playFrom(filteredTracks, track)} />
                {/each}
            </div>
        {/if}

        {#if tab !== 'songs' && recentTracks.length}
            <SectionLabel class="mb-2">{t('library.recentlyAdded')}</SectionLabel>
            <div class="rounded-card overflow-hidden border border-border">
                {#each recentTracks as track, i (track.id)}
                    <TrackRow {track} index={i} onclick={() => playFrom(recentTracks, track)} />
                {/each}
            </div>
        {/if}
    {/if}
</div>
