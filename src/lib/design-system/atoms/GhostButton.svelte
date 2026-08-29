<script lang="ts">
    import type { Snippet } from 'svelte';

    // Subdued pill action — pairs with PrimaryButton (Rescan next to Add
    // folder, Reset in the EQ sheet).
    interface Props {
        children: Snippet;
        onclick?: () => void;
        icon?: string;
    }

    let { children, onclick, icon }: Props = $props();
</script>

<button
    class="inline-flex items-center gap-2 cursor-pointer text-body font-bold text-text-muted px-3.5 py-2 border border-border-emphasis rounded-full transition-colors duration-fast hover:text-text hover:bg-chip"
    {onclick}
>
    {#if icon}
        <svg
            class="size-3.75"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <path d={icon} />
        </svg>
    {/if}
    {@render children()}
</button>
