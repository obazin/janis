<script lang="ts">
    import { eqOpen } from '$lib/stores/EqOverlayStore';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import type { TranslationKey } from '$lib/i18n/types';

    // The one "open the equalizer" affordance — an accent pill with the EQ
    // glyph. Extracted because it used to be hand-written wherever it
    // appeared, and the copies had already drifted apart in padding, border
    // and hover treatment.
    interface Props {
        labelKey: TranslationKey;
    }

    let { labelKey }: Props = $props();
</script>

<button
    class="flex items-center gap-1.75 flex-none cursor-pointer text-caption font-bold text-accent px-3.5 py-2 border border-accent/35 rounded-full transition-colors duration-fast hover:bg-accent/12"
    onclick={() => eqOpen.set(true)}
>
    <svg
        class="size-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
    >
        <path d={ICONS.equalizer} />
    </svg>
    {t(labelKey)}
</button>
