<script lang="ts">
    import { libraryStore } from '$lib/features/library/LibraryStore.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import ShelfTile from '$lib/features/library/ShelfTile.svelte';
    import Eyebrow from '$lib/design-system/atoms/Eyebrow.svelte';
    import EmptyState from '$lib/design-system/atoms/EmptyState.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // Shelves are built from the user's own library — no external catalogue.
    const recentAlbums = $derived(libraryStore.albums.slice(0, 8));
    const topArtists = $derived(libraryStore.artists.slice(0, 8));
    const rediscover = $derived([...libraryStore.albums].reverse().slice(0, 8));
</script>

<div class="px-11 pt-9 pb-10 animate-float-up">
    <Eyebrow color="teal" class="mb-2">{t('discover.eyebrow')}</Eyebrow>
    <h1 class="font-display font-black text-display tracking-tight m-0 mb-7">
        {t('discover.title')}
    </h1>

    {#if libraryStore.albums.length === 0}
        <EmptyState
            icon={ICONS.discover}
            title={t('discover.empty.title')}
            description={t('discover.empty.desc')}
            cta={{
                label: t('library.addMusic'),
                icon: ICONS.plus,
                onclick: () => libraryStore.addFiles(),
            }}
        />
    {:else}
        {#each [{ title: t('discover.recentAlbums'), albums: recentAlbums }, { title: t('discover.rediscover'), albums: rediscover }] as shelf (shelf.title)}
            <div class="mb-9">
                <div class="font-display font-extrabold text-heading-md mb-3.5">
                    {shelf.title}
                </div>
                <div class="flex gap-4.5 overflow-x-auto pb-2 mscroll">
                    {#each shelf.albums as album (album.key)}
                        <ShelfTile
                            title={album.album ?? t('common.unknownAlbum')}
                            subtitle={album.artist ?? t('common.unknownArtist')}
                            seed="{album.artist ?? ''}-{album.album ?? ''}"
                            onclick={() => playerStore.playQueue(album.tracks, 0)}
                        />
                    {/each}
                </div>
            </div>
        {/each}
        {#if topArtists.length}
            <div class="mb-9">
                <div class="font-display font-extrabold text-heading-md mb-3.5">
                    {t('discover.artists')}
                </div>
                <div class="flex gap-4.5 overflow-x-auto pb-2 mscroll">
                    {#each topArtists as artist (artist.artist)}
                        <ShelfTile
                            title={artist.artist}
                            subtitle={t('library.tracks', { count: artist.tracks.length })}
                            seed={artist.artist}
                            onclick={() => playerStore.playQueue(artist.tracks, 0)}
                        />
                    {/each}
                </div>
            </div>
        {/if}
    {/if}
</div>
