<script lang="ts">
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import { libraryStore } from '$lib/features/library/LibraryStore.svelte';
    import { qualityLabel, artInitials, fmtTime } from '$lib/features/player/format';
    import WaveformCanvas from '$lib/features/player/WaveformCanvas.svelte';
    import NowPlayingQueue from '$lib/features/player/NowPlayingQueue.svelte';
    import ArtistSpotlight from '$lib/features/library/ArtistSpotlight.svelte';
    import TransportControls from '$lib/features/player/TransportControls.svelte';
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';
    import Badge from '$lib/design-system/atoms/Badge.svelte';
    import Eyebrow from '$lib/design-system/atoms/Eyebrow.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import EmptyState from '$lib/design-system/atoms/EmptyState.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    const track = $derived(playerStore.current);
    const station = $derived(playerStore.station);
    // A station announcing a track wins the hero slot — the station name is
    // already in the eyebrow, and what is playing is the more useful headline.
    const title = $derived(track?.title ?? playerStore.streamTitle ?? station?.name ?? '');
    const artSeed = $derived(
        track ? `${track.artist ?? ''}-${track.album ?? track.title}` : (station?.id ?? 'janis'),
    );
</script>

{#if !track && !station}
    <div class="px-11 py-9 animate-float-up">
        <EmptyState
            icon={ICONS.nowPlaying}
            title={t('now.empty.title')}
            description={t('now.empty.desc')}
            cta={{
                label: t('now.empty.cta'),
                icon: ICONS.plus,
                onclick: () => libraryStore.addFiles(),
            }}
        />
    </div>
{:else}
    <div
        class="px-11 pt-9 pb-10 animate-float-up grid grid-cols-[minmax(300px,380px)_1fr] gap-12 items-start min-h-full"
    >
        <div class="flex flex-col items-center gap-5.5 sticky top-0">
            <div class="relative w-full aspect-square">
                <div
                    class="art-halo rounded-full bg-prism-conic blur-3xl opacity-50 animate-spin-slow"
                ></div>
                <ArtTile
                    seed={artSeed}
                    coverUrl={playerStore.coverUrl}
                    initials={artInitials(title)}
                    gradIndex={station?.gradIndex}
                    class="absolute inset-0 rounded-art shadow-art ring-1 ring-inset ring-border-emphasis"
                />
            </div>
            <div class="flex gap-2.5 flex-wrap justify-center">
                {#if track}
                    <Badge variant="soft">{qualityLabel(track)}</Badge>
                    {#if track.lossless}
                        <Badge variant="tint" tone="lime">{t('now.lossless')}</Badge>
                    {/if}
                {:else if station}
                    <Badge variant="soft">{t(station.genreKey)}</Badge>
                    <Badge variant="tint" tone="ember">{t('radio.live')}</Badge>
                {/if}
            </div>
        </div>

        <div class="flex flex-col gap-5.5 min-w-0">
            <div>
                <Eyebrow color="accent" class="mb-3">
                    {#if track}
                        {track.album
                            ? t('now.eyebrow', { album: track.album })
                            : t('now.eyebrowNoAlbum')}
                    {:else if station}
                        {t('now.eyebrowRadio', { station: station.name })}
                    {/if}
                </Eyebrow>
                <h1
                    class="font-display font-black text-hero leading-hero tracking-tight text-balance m-0"
                >
                    {title}
                </h1>
            </div>

            <div class="flex gap-10 flex-wrap">
                {#if track}
                    <div>
                        <SectionLabel class="mb-1">{t('now.artist')}</SectionLabel>
                        <div class="text-heading-md font-bold">
                            {track.artist ?? t('common.unknownArtist')}
                        </div>
                    </div>
                    {#if track.composer}
                        <div>
                            <SectionLabel class="mb-1">{t('now.composer')}</SectionLabel>
                            <div class="text-heading-md font-bold text-teal">{track.composer}</div>
                        </div>
                    {/if}
                    <div>
                        <SectionLabel class="mb-1">{t('now.album')}</SectionLabel>
                        <div class="text-heading-md font-semibold text-text-secondary">
                            {track.album ?? t('common.unknownAlbum')}
                        </div>
                    </div>
                {:else if station}
                    <div>
                        <SectionLabel class="mb-1">{t('now.station')}</SectionLabel>
                        <div class="text-heading-md font-bold">{station.name}</div>
                    </div>
                    <div>
                        <SectionLabel class="mb-1">{t('radio.title')}</SectionLabel>
                        <div class="text-heading-md font-bold">{t(station.genreKey)}</div>
                    </div>
                {/if}
            </div>

            <div>
                <SectionLabel class="mb-2">{t('now.waveform')}</SectionLabel>
                <div
                    class="relative h-30 bg-well rounded-card border border-border overflow-hidden"
                >
                    <WaveformCanvas variant="big" />
                </div>
                <div class="flex justify-between mt-2 font-mono text-caption text-text-muted">
                    <span>{fmtTime(playerStore.currentTime)}</span>
                    <span>{track ? fmtTime(playerStore.duration) : t('radio.live')}</span>
                </div>
            </div>

            <TransportControls />

            <NowPlayingQueue />

            {#if track}
                <ArtistSpotlight artist={track.artist} />
            {/if}
        </div>
    </div>
{/if}
