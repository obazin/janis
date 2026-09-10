<script lang="ts">
    import type { Track } from '$lib/models/Track';
    import TrackRow from './TrackRow.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import IconButton from '$lib/design-system/atoms/IconButton.svelte';
    import { IconButtonIdentifier } from '$lib/design-system/atoms/IconButtonIdentifier';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // The track list for a selected album/artist, expanded in place right
    // under the grid tile that opened it — spans the full grid width
    // (`col-span-full`) so it lands on its own row instead of at the bottom
    // of a long grid (see CLAUDE.md rule 12: the selection itself lives in
    // `libraryViewStore`, this component just renders it).
    interface Props {
        heading: string;
        tracks: Track[];
        /** Album tracks show their own disc position; artist tracks don't. */
        useTrackNumber?: boolean;
        onPlay: (track: Track) => void;
        onClose: () => void;
    }

    let { heading, tracks, useTrackNumber = false, onPlay, onClose }: Props = $props();

    // Selecting a different tile while one is already open unmounts the old
    // panel and mounts this one fresh — which can shift the grid enough that
    // the new panel lands outside the scrolled viewport. Bring it into view
    // once, on mount.
    function revealOnMount(panel: HTMLDivElement) {
        panel.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
</script>

<div
    {@attach revealOnMount}
    class="col-span-full rounded-card overflow-hidden border border-border bg-panel animate-float-up"
>
    <div class="flex items-center justify-between px-4 py-3 border-b border-border">
        <SectionLabel>{heading}</SectionLabel>
        <div class="size-8.5 rounded-full bg-chip-hover flex items-center justify-center">
            <IconButton
                identifier={IconButtonIdentifier.LibraryCollapseSelection}
                d={ICONS.close}
                label={t('library.collapseSelection')}
                onclick={onClose}
            />
        </div>
    </div>
    {#each tracks as track, i (track.id)}
        <TrackRow {track} index={i} {useTrackNumber} onclick={() => onPlay(track)} />
    {/each}
</div>
