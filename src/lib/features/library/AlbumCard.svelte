<script lang="ts">
    import CoverArt from './CoverArt.svelte';

    // Library grid card: art + title + subtitle.
    interface Props {
        title: string;
        subtitle: string;
        seed: string;
        /** Track to read embedded art from — normally the album's first. */
        coverTrackId?: number | null;
        /** Extra line under the subtitle: year, track count, runtime. */
        detail?: string | null;
        /** Selected state — an accent ring, matching the app's other
         *  selection treatments (NavItem, the Now Playing queue rows). */
        active?: boolean;
        onclick: () => void;
    }

    let {
        title,
        subtitle,
        seed,
        coverTrackId = null,
        detail = null,
        active = false,
        onclick,
    }: Props = $props();
</script>

<button
    class="cursor-pointer p-3.5 rounded-panel bg-panel border text-left transition-all duration-base hover:bg-panel-hover hover:-translate-y-0.75
    {active ? 'border-accent ring-1 ring-inset ring-accent/40' : 'border-border'}"
    {onclick}
>
    <CoverArt {seed} trackId={coverTrackId} class="aspect-square rounded-xl mb-3 shadow-card" />
    <div class="font-bold text-heading-sm truncate">{title}</div>
    <div class="text-body text-text-muted mt-0.5 truncate">{subtitle}</div>
    {#if detail}
        <div class="text-caption text-text-faint mt-0.5 truncate">{detail}</div>
    {/if}
</button>
