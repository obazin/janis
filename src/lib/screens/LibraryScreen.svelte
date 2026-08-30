<script lang="ts">
    import {
        libraryStore,
        type AlbumGroup,
        type ArtistGroup,
    } from '$lib/features/library/LibraryStore.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import { fmtTime } from '$lib/features/player/format';
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

    // Clicking an album or artist tile browses it rather than playing it —
    // the list below (otherwise "Recently added") shows its tracks until
    // the selection is cleared or the tab changes. A playlist selection
    // will join this union once playlists have real data to select.
    type Selection = { kind: 'album' | 'artist'; key: string; label: string; tracks: Track[] };
    let selection = $state<Selection | null>(null);

    function selectTab(candidate: Tab) {
        tab = candidate;
        selection = null;
    }

    function toggleSelection(next: Selection) {
        // Re-clicking the selected tile is how you back out of it, short of
        // clearing it explicitly or switching tabs.
        selection = selection?.kind === next.kind && selection.key === next.key ? null : next;
    }

    function selectAlbum(album: AlbumGroup) {
        toggleSelection({
            kind: 'album',
            key: album.key,
            label: album.album ?? t('common.unknownAlbum'),
            tracks: album.tracks,
        });
    }

    function selectArtist(artist: ArtistGroup) {
        toggleSelection({
            kind: 'artist',
            key: artist.artist,
            label: artist.artist,
            tracks: artist.tracks,
        });
    }

    function matches(track: Track, query: string): boolean {
        return [
            track.title,
            track.artist,
            track.album,
            track.composer,
            track.albumArtist,
            track.genre,
        ]
            .filter((v): v is string => v != null)
            .some((v) => v.toLowerCase().includes(query));
    }

    const query = $derived($searchQuery.trim().toLowerCase());
    const filteredTracks = $derived(
        query ? libraryStore.tracks.filter((tr) => matches(tr, query)) : [...libraryStore.tracks],
    );
    // Year · track count · runtime, dropping whatever the tags did not say.
    function albumDetail(album: AlbumGroup): string {
        return [
            album.year ? String(album.year) : null,
            t('library.tracks', { count: album.tracks.length }),
            fmtTime(album.durationSecs),
        ]
            .filter(Boolean)
            .join(' · ');
    }

    const filteredAlbums = $derived(
        libraryStore.albums.filter((a) => a.tracks.some((tr) => filteredTracks.includes(tr))),
    );
    const filteredArtists = $derived(
        libraryStore.artists.filter((a) => a.tracks.some((tr) => filteredTracks.includes(tr))),
    );
    const recentTracks = $derived(filteredTracks.slice(0, 8));
    // What the list below the grid shows: the selected album/artist's own
    // tracks, or "Recently added" when nothing is selected.
    const listTracks = $derived(selection ? selection.tracks : recentTracks);
    const listHeading = $derived(selection ? selection.label : t('library.recentlyAdded'));

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
                    onclick={() => selectTab(candidate)}
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
                        coverTrackId={album.tracks[0]?.id ?? null}
                        detail={albumDetail(album)}
                        active={selection?.kind === 'album' && selection.key === album.key}
                        onclick={() => selectAlbum(album)}
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
                        coverTrackId={artist.tracks[0]?.id ?? null}
                        active={selection?.kind === 'artist' && selection.key === artist.artist}
                        onclick={() => selectArtist(artist)}
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

        {#if tab !== 'songs' && listTracks.length}
            <div class="flex items-center justify-between mb-2">
                <SectionLabel>{listHeading}</SectionLabel>
                {#if selection}
                    <button
                        class="cursor-pointer text-caption font-bold text-accent transition-opacity duration-fast hover:opacity-80"
                        onclick={() => (selection = null)}
                    >
                        {t('library.backToRecentlyAdded')}
                    </button>
                {/if}
            </div>
            <div class="rounded-card overflow-hidden border border-border">
                {#each listTracks as track, i (track.id)}
                    <TrackRow
                        {track}
                        index={i}
                        useTrackNumber={selection?.kind === 'album'}
                        onclick={() => playFrom(listTracks, track)}
                    />
                {/each}
            </div>
        {/if}
    {/if}
</div>
