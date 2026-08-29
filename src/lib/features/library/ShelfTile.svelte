<script lang="ts">
    import CoverArt from './CoverArt.svelte';

    // Discover shelf tile: bare art + two text lines (no card chrome).
    interface Props {
        title: string;
        subtitle: string;
        seed: string;
        /** Track to read embedded art from — normally the album's first. */
        coverTrackId?: number | null;
        onclick: () => void;
    }

    let { title, subtitle, seed, coverTrackId = null, onclick }: Props = $props();
</script>

<button class="w-44 flex-none cursor-pointer text-left" {onclick}>
    <CoverArt {seed} trackId={coverTrackId} class="aspect-square rounded-card mb-2.5 shadow-tile" />
    <div class="font-bold text-body-em truncate">{title}</div>
    <div class="text-caption text-text-muted truncate">{subtitle}</div>
</button>
