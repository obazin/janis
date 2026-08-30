<script lang="ts">
    import {
        libraryStore,
        type AlbumGroup,
        type ArtistGroup,
    } from '$lib/features/library/LibraryStore.svelte';
    import {
        libraryViewStore,
        LIBRARY_TABS,
        type LibraryTab,
        type LibrarySelection,
    } from '$lib/features/library/LibraryViewStore.svelte';
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

    const TAB_KEYS: Record<LibraryTab, TranslationKey> = {
        playlists: 'library.tab.playlists',
        artists: 'library.tab.artists',
        albums: 'library.tab.albums',
        songs: 'library.tab.songs',
    };

    // Tab and selection live in `libraryViewStore`, not screen-local `$state`
    // — this screen component is destroyed and recreated on every
    // navigation, and a selection is meant to survive that (CLAUDE.md rule
    // 12): leaving Library for another screen and coming back should still
    // show the album or artist you were browsing.

    function selectTab(candidate: LibraryTab) {
        libraryViewStore.tab = candidate;
        libraryViewStore.selection = null;
    }

    function toggleSelection(next: NonNullable<LibrarySelection>) {
        // Re-clicking the selected tile is how you back out of it, short of
        // clearing it explicitly or switching tabs.
        const current = libraryViewStore.selection;
        libraryViewStore.selection =
            current?.kind === next.kind && current.key === next.key ? null : next;
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
    const listTracks = $derived(
        libraryViewStore.selection ? libraryViewStore.selection.tracks : recentTracks,
    );
    const listHeading = $derived(
        libraryViewStore.selection ? libraryViewStore.selection.label : t('library.recentlyAdded'),
    );

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
            {#each LIBRARY_TABS as candidate (candidate)}
                <Chip
                    label={t(TAB_KEYS[candidate])}
                    active={libraryViewStore.tab === candidate}
                    onclick={() => selectTab(candidate)}
                />
            {/each}
        </div>

        {#if libraryViewStore.tab === 'playlists'}
            <EmptyState
                icon={ICONS.library}
                title={t('library.playlistsSoon.title')}
                description={t('library.playlistsSoon.desc')}
            />
        {:else if libraryViewStore.tab === 'albums'}
            <div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-5 mb-9">
                {#each filteredAlbums as album (album.key)}
                    <AlbumCard
                        title={album.album ?? t('common.unknownAlbum')}
                        subtitle={album.artist ?? t('common.unknownArtist')}
                        seed="{album.artist ?? ''}-{album.album ?? ''}"
                        coverTrackId={album.tracks[0]?.id ?? null}
                        detail={albumDetail(album)}
                        active={libraryViewStore.selection?.kind === 'album' &&
                            libraryViewStore.selection.key === album.key}
                        onclick={() => selectAlbum(album)}
                    />
                {/each}
            </div>
        {:else if libraryViewStore.tab === 'artists'}
            <div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-5 mb-9">
                {#each filteredArtists as artist (artist.artist)}
                    <AlbumCard
                        title={artist.artist}
                        subtitle={t('library.tracks', { count: artist.tracks.length })}
                        seed={artist.artist}
                        coverTrackId={artist.tracks[0]?.id ?? null}
                        active={libraryViewStore.selection?.kind === 'artist' &&
                            libraryViewStore.selection.key === artist.artist}
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

        {#if libraryViewStore.tab !== 'songs' && listTracks.length}
            <div class="flex items-center justify-between mb-2">
                <SectionLabel>{listHeading}</SectionLabel>
                {#if libraryViewStore.selection}
                    <button
                        class="cursor-pointer text-caption font-bold text-accent transition-opacity duration-fast hover:opacity-80"
                        onclick={() => (libraryViewStore.selection = null)}
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
                        useTrackNumber={libraryViewStore.selection?.kind === 'album'}
                        onclick={() => playFrom(listTracks, track)}
                    />
                {/each}
            </div>
        {/if}
    {/if}
</div>
