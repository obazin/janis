<script lang="ts">
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';
    import { libraryStore } from './LibraryStore.svelte';

    // An `ArtTile` that fills itself in with the track's real embedded art.
    //
    // The art is only requested once the tile is on screen. It crosses IPC as
    // base64, so a Songs tab listing hundreds of tracks would otherwise pull
    // every cover in the library the moment it rendered. Until it arrives —
    // and for files that carry none — `ArtTile` draws its prism gradient, so
    // there is nothing to wait for and no flash of empty box.
    interface Props {
        /** Track to read art from, or `null` to stay a gradient. */
        trackId?: number | null;
        seed: string;
        initials?: string | null;
        gradIndex?: number;
        class?: string;
    }

    let { trackId = null, seed, initials = null, gradIndex, class: extra = '' }: Props = $props();

    let box = $state<HTMLElement | null>(null);
    let onScreen = $state(false);

    $effect(() => {
        if (!box || onScreen || trackId === null) return;
        const observer = new IntersectionObserver(
            (entries) => {
                if (entries.some((entry) => entry.isIntersecting)) onScreen = true;
            },
            // Start a little before the fold, so art is there by the time the
            // tile is.
            { rootMargin: '200px' },
        );
        observer.observe(box);
        return () => observer.disconnect();
    });

    const coverUrl = $derived(onScreen && trackId !== null ? libraryStore.coverFor(trackId) : null);
</script>

<!-- The wrapper carries the caller's sizing so the observer has a box to
     watch; the tile fills it and keeps the same classes for its own rounding
     and clipping. -->
<div bind:this={box} class={extra}>
    <ArtTile {seed} {coverUrl} {initials} {gradIndex} class="size-full {extra}" />
</div>
