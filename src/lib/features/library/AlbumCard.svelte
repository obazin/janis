<script lang="ts">
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';
    import { libraryStore } from './LibraryStore.svelte';

    // Library grid card: art + title + subtitle.
    //
    // Real cover art is pulled from the file, but only once the tile is on
    // screen: art crosses IPC as base64, and a library with hundreds of albums
    // would otherwise fetch every cover the moment the grid renders.
    interface Props {
        title: string;
        subtitle: string;
        seed: string;
        /** Track to read embedded art from — normally the album's first. */
        coverTrackId?: number | null;
        /** Extra line under the subtitle: year, track count, runtime. */
        detail?: string | null;
        onclick: () => void;
    }

    let { title, subtitle, seed, coverTrackId = null, detail = null, onclick }: Props = $props();

    let card = $state<HTMLElement | null>(null);
    let visible = $state(false);

    $effect(() => {
        if (!card || visible || coverTrackId === null) return;
        const observer = new IntersectionObserver(
            (entries) => {
                if (entries.some((e) => e.isIntersecting)) visible = true;
            },
            // A little ahead of the fold, so art is there by the time it is.
            { rootMargin: '200px' },
        );
        observer.observe(card);
        return () => observer.disconnect();
    });

    const coverUrl = $derived(
        visible && coverTrackId !== null ? libraryStore.coverFor(coverTrackId) : null,
    );
</script>

<button
    bind:this={card}
    class="cursor-pointer p-3.5 rounded-panel bg-panel border border-border text-left transition-all duration-base hover:bg-panel-hover hover:-translate-y-0.75"
    {onclick}
>
    <ArtTile {seed} {coverUrl} class="aspect-square rounded-xl mb-3 shadow-card" />
    <div class="font-bold text-heading-sm truncate">{title}</div>
    <div class="text-body text-text-muted mt-0.5 truncate">{subtitle}</div>
    {#if detail}
        <div class="text-caption text-text-faint mt-0.5 truncate">{detail}</div>
    {/if}
</button>
