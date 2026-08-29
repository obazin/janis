<script lang="ts">
    import PrimaryButton from './PrimaryButton.svelte';

    // Centered icon + title + description + optional CTA.
    interface Props {
        icon: string;
        title: string;
        description: string;
        cta?: { label: string; icon?: string; onclick: () => void };
    }

    let { icon, title, description, cta }: Props = $props();
</script>

<div class="flex flex-col items-center justify-center gap-3.5 py-16 text-center">
    <svg
        class="size-11.5 text-text-faint"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linecap="round"
        stroke-linejoin="round"
    >
        <path d={icon} />
    </svg>
    <div class="font-display font-extrabold text-heading-md">{title}</div>
    <p class="text-body-em text-text-muted max-w-100 leading-normal m-0">{description}</p>
    {#if cta}
        <div class="mt-2.5">
            <PrimaryButton icon={cta.icon} onclick={cta.onclick}>{cta.label}</PrimaryButton>
        </div>
    {/if}
</div>
