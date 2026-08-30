<script lang="ts">
    import { playerStore } from './PlayerStore.svelte';
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';
    import IconButton from '$lib/design-system/atoms/IconButton.svelte';
    import PlayCircle from '$lib/design-system/molecules/PlayCircle.svelte';
    import { IconButtonIdentifier } from '$lib/design-system/atoms/IconButtonIdentifier';
    import WaveformCanvas from './WaveformCanvas.svelte';
    import OpenEqButton from '$lib/features/eq/OpenEqButton.svelte';
    import { navigateTo } from '$lib/stores/NavigationStore.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // The persistent bottom bar: current art + titles, compact transport,
    // mini waveform, EQ shortcut.

    const title = $derived(playerStore.current?.title ?? playerStore.station?.name ?? '—');
    const subtitle = $derived(
        playerStore.current
            ? (playerStore.current.artist ?? t('common.unknownArtist'))
            : playerStore.station
              ? t(playerStore.station.genreKey)
              : '',
    );
    const artSeed = $derived(
        playerStore.current
            ? `${playerStore.current.artist ?? ''}-${playerStore.current.album ?? playerStore.current.title}`
            : (playerStore.station?.id ?? 'janis'),
    );
</script>

<div
    class="h-19 flex-none flex items-center gap-4.5 px-5 bg-miniplayer backdrop-blur-xl border-t border-border"
>
    <button
        class="flex items-center gap-3.25 w-62.5 cursor-pointer text-left"
        onclick={() => navigateTo('now-playing')}
    >
        <ArtTile
            seed={artSeed}
            coverUrl={playerStore.coverUrl}
            gradIndex={playerStore.station?.gradIndex}
            class="size-12 rounded-thumb flex-none"
        />
        <div class="min-w-0">
            <div class="font-bold text-body-em truncate">{title}</div>
            <div class="text-caption text-text-muted truncate">{subtitle}</div>
        </div>
    </button>
    <div class="flex items-center gap-4">
        <IconButton
            identifier={IconButtonIdentifier.MiniPrevious}
            d={ICONS.previous}
            label={t('player.previous')}
            mode="fill"
            tone="secondary"
            onclick={() => playerStore.previous()}
        />
        <PlayCircle
            identifier={IconButtonIdentifier.MiniPlayPause}
            variant="mini"
            playing={playerStore.playing}
            label={playerStore.playing ? t('player.pause') : t('player.play')}
            onclick={() => playerStore.toggle()}
        />
        <IconButton
            identifier={IconButtonIdentifier.MiniNext}
            d={ICONS.next}
            label={t('player.next')}
            mode="fill"
            tone="secondary"
            onclick={() => playerStore.next()}
        />
    </div>
    <div class="flex-1 h-11 min-w-0">
        <WaveformCanvas variant="mini" />
    </div>
    <OpenEqButton labelKey="player.eq" />
</div>
