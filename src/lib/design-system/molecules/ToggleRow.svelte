<script lang="ts">
    import Toggle from '../atoms/Toggle.svelte';

    // Settings row: label + description on the left, a Toggle on the right.
    // Composed inside a bordered list container by the caller.
    interface Props {
        label: string;
        description: string;
        checked: boolean;
        onToggle: () => void;
    }

    let { label, description, checked, onToggle }: Props = $props();
</script>

<div
    class="flex items-center justify-between gap-4 px-4.5 py-3.75 border-b border-divider last:border-b-0"
>
    <div>
        <div class="font-semibold text-heading-sm">{label}</div>
        <div class="text-caption text-text-muted">{description}</div>
    </div>
    <Toggle {checked} {onToggle} {label} />
</div>
