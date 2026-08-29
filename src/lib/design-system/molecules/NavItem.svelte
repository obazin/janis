<script lang="ts">
    // One sidebar row: icon + label. Content-agnostic — caller supplies the
    // pre-translated label and the SVG path data.
    interface Props {
        label: string;
        d: string;
        active?: boolean;
        onclick: () => void;
    }

    let { label, d, active = false, onclick }: Props = $props();
</script>

<button
    class="flex items-center gap-3 px-3 py-2.75 rounded-btn cursor-pointer text-body-em font-semibold text-left w-full transition-colors duration-fast select-none
    {active
        ? 'bg-nav-active text-text ring-1 ring-inset ring-accent/35'
        : 'text-nav-idle hover:text-text-soft hover:bg-panel'}"
    {onclick}
>
    <svg
        class="size-4.75 flex-none {active ? 'text-accent' : 'text-nav-idle'}"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.9"
        stroke-linecap="round"
        stroke-linejoin="round"
    >
        <path {d} />
    </svg>
    <span>{label}</span>
</button>
