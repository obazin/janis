<script lang="ts">
    // Switch-style toggle. Controlled: caller owns `checked` + `onToggle`.
    interface Props {
        checked: boolean;
        onToggle: () => void;
        label: string;
    }

    let { checked, onToggle, label }: Props = $props();
</script>

<button
    role="switch"
    aria-checked={checked}
    aria-label={label}
    class="relative w-11.5 h-6.5 rounded-full cursor-pointer transition-colors duration-base flex-none
    {checked ? 'bg-accent' : 'bg-rail'}"
    onclick={onToggle}
>
    <span
        class="absolute top-0.75 size-5 rounded-full bg-text transition-all duration-base
        {checked ? 'left-5.75' : 'left-0.75'}"
    ></span>
</button>
