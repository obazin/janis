<script lang="ts">
    import type { Track } from '$lib/models/Track';
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';
    import { fmtTime } from '$lib/features/player/format';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // One "Recently added" row: number · art · title/artist · composer ·
    // duration.
    interface Props {
        track: Track;
        index: number;
        onclick: () => void;
    }

    let { track, index, onclick }: Props = $props();
</script>

<button
    class="flex gap-4 items-center px-3.5 py-2.75 cursor-pointer w-full text-left transition-colors duration-fast hover:bg-accent/8
    {index > 0 ? 'border-t border-divider' : ''}"
    {onclick}
>
    <div class="w-6.5 text-center font-mono text-body text-text-faint">
        {String(index + 1).padStart(2, '0')}
    </div>
    <ArtTile
        seed="{track.artist ?? ''}-{track.album ?? track.title}"
        class="size-9.5 rounded-lg flex-none"
    />
    <div class="flex-1 min-w-0">
        <div class="font-semibold text-body-em truncate">{track.title}</div>
        <div class="text-caption text-text-muted truncate">
            {track.artist ?? t('common.unknownArtist')}
        </div>
    </div>
    <div class="w-35 text-body text-text-muted truncate">{track.composer ?? ''}</div>
    <div class="w-17.5 font-mono text-caption text-text-muted text-right">
        {fmtTime(track.durationSecs)}
    </div>
</button>
