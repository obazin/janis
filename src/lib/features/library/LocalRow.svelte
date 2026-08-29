<script lang="ts">
    import type { Track } from '$lib/models/Track';
    import { fmtTime } from '$lib/features/player/format';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // One Local Files table row: title · artist · format chip · duration.
    interface Props {
        track: Track;
        first: boolean;
        onclick: () => void;
    }

    let { track, first, onclick }: Props = $props();
</script>

<button
    class="flex gap-4 items-center px-4 py-2.75 cursor-pointer w-full text-left text-body-em transition-colors duration-fast hover:bg-lime/7
    {first ? '' : 'border-t border-divider'}"
    {onclick}
>
    <div class="flex-1 min-w-0 truncate font-semibold">{track.title}</div>
    <div class="w-37.5 text-text-muted truncate">
        {track.artist ?? t('common.unknownArtist')}
    </div>
    <div class="w-20">
        <span class="font-mono text-label text-lime border border-lime/30 rounded-sm px-1.5 py-0.5">
            {track.format}
        </span>
    </div>
    <div class="w-17.5 text-right font-mono text-caption text-text-muted">
        {fmtTime(track.durationSecs)}
    </div>
</button>
