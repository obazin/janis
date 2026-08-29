<script lang="ts">
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';

    // Library grid card: art + title + subtitle.
    interface Props {
        title: string;
        subtitle: string;
        seed: string;
        onclick: () => void;
    }

    let { title, subtitle, seed, onclick }: Props = $props();
</script>

<button
    class="cursor-pointer p-3.5 rounded-panel bg-panel border border-border text-left transition-all duration-base hover:bg-panel-hover hover:-translate-y-0.75"
    {onclick}
>
    <ArtTile {seed} class="aspect-square rounded-xl mb-3 shadow-card" />
    <div class="font-bold text-heading-sm truncate">{title}</div>
    <div class="text-body text-text-muted mt-0.5 truncate">{subtitle}</div>
</button>
