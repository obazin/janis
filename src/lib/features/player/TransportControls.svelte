<script lang="ts">
    import { playerStore } from './PlayerStore.svelte';
    import IconButton from '$lib/design-system/atoms/IconButton.svelte';
    import PlayCircle from '$lib/design-system/molecules/PlayCircle.svelte';
    import { IconButtonIdentifier } from '$lib/design-system/atoms/IconButtonIdentifier';
    import VolumeSlider from './VolumeSlider.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // The Now Playing transport row: shuffle · prev · play · next · repeat,
    // then volume on the right.
</script>

<div class="flex items-center gap-5">
    <IconButton
        identifier={IconButtonIdentifier.TransportShuffle}
        d={ICONS.shuffle}
        label={t('player.shuffle')}
        active={playerStore.shuffle}
        tone="muted"
        onclick={() => playerStore.toggleShuffle()}
    />
    <IconButton
        identifier={IconButtonIdentifier.TransportPrevious}
        d={ICONS.previous}
        label={t('player.previous')}
        mode="fill"
        size="md"
        onclick={() => playerStore.previous()}
    />
    <PlayCircle
        identifier={IconButtonIdentifier.TransportPlayPause}
        playing={playerStore.playing}
        label={playerStore.playing ? t('player.pause') : t('player.play')}
        onclick={() => playerStore.toggle()}
    />
    <IconButton
        identifier={IconButtonIdentifier.TransportNext}
        d={ICONS.next}
        label={t('player.next')}
        mode="fill"
        size="md"
        onclick={() => playerStore.next()}
    />
    <IconButton
        identifier={IconButtonIdentifier.TransportRepeat}
        d={ICONS.repeat}
        label={t('player.repeat')}
        active={playerStore.repeat}
        tone="muted"
        onclick={() => playerStore.toggleRepeat()}
    />
    <div class="flex-1"></div>
    <svg
        class="size-4.75 text-text-muted"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
    >
        <path d={ICONS.volume} />
    </svg>
    <VolumeSlider />
</div>
