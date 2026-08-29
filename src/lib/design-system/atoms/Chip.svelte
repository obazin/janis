<script lang="ts">
    // Selectable pill — EQ presets, library tabs, radio genre filters.
    interface Props {
        label: string;
        active?: boolean;
        onclick?: () => void;
    }

    let { label, active = false, onclick }: Props = $props();
</script>

<button
    class="cursor-pointer text-body font-bold px-3.75 py-2 rounded-full transition-colors duration-fast select-none
    {active
        ? 'bg-accent text-on-accent'
        : 'bg-chip text-text-soft border border-border-strong hover:bg-chip-hover'}"
    {onclick}
>
    {label}
</button>
