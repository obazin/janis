<script lang="ts">
    import { eqStore } from './EqStore.svelte';
    import { PRESET_ORDER, PRESET_LABEL_KEYS, type EqPresetName } from './presets';
    import { FREQ_LABELS } from './bands';
    import EqBandSlider from './EqBandSlider.svelte';
    import Chip from '$lib/design-system/atoms/Chip.svelte';
    import ToggleRow from '$lib/design-system/molecules/ToggleRow.svelte';
    import IconButton from '$lib/design-system/atoms/IconButton.svelte';
    import { IconButtonIdentifier } from '$lib/design-system/atoms/IconButtonIdentifier';
    import { eqOpen } from '$lib/stores/EqOverlayStore';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // The 10-band EQ bottom sheet. Rendered by the layout above everything;
    // visible while the `eqOpen` channel is true. Scrim click and Escape
    // close it; clicks inside the sheet stay inside.

    // Once the mode is on the engine reports what it actually costs at the
    // open device's rate, so the row names the real figure instead of the
    // generic promise.
    const linearPhaseDescription = $derived(
        eqStore.linearPhase && eqStore.latencySecs > 0
            ? t('eq.linearPhase.latency', { ms: Math.round(eqStore.latencySecs * 1000) })
            : t('eq.linearPhase.desc'),
    );

    function close() {
        eqOpen.set(false);
    }

    function onKeydown(event: KeyboardEvent) {
        if (event.key === 'Escape' && $eqOpen) close();
    }
</script>

<svelte:window onkeydown={onKeydown} />

{#if $eqOpen}
    <div
        class="absolute inset-0 z-overlay bg-scrim backdrop-blur-sm flex items-end justify-center animate-float-up"
        role="presentation"
        onclick={close}
        onkeydown={(event) => event.key === 'Enter' && close()}
    >
        <div
            class="w-full max-w-215 mx-auto bg-eq-sheet border border-border-emphasis rounded-t-art px-8 pt-6.5 pb-8.5 shadow-sheet"
            role="dialog"
            tabindex="-1"
            aria-label={t('eq.title')}
            onclick={(event) => event.stopPropagation()}
            onkeydown={(event) => event.stopPropagation()}
        >
            <div class="flex items-center justify-between mb-1.5">
                <div>
                    <div class="font-display font-black text-heading-lg">{t('eq.title')}</div>
                    <div class="text-body text-text-muted">{t('eq.subtitle')}</div>
                </div>
                <div class="flex items-center gap-3.5">
                    <button
                        class="cursor-pointer text-body font-bold text-text-muted px-3.5 py-2 border border-border-emphasis rounded-full transition-colors duration-fast hover:text-text"
                        onclick={() => eqStore.reset()}
                    >
                        {t('eq.reset')}
                    </button>
                    <div
                        class="size-8.5 rounded-full bg-chip-hover flex items-center justify-center"
                    >
                        <IconButton
                            identifier={IconButtonIdentifier.EqClose}
                            d={ICONS.close}
                            label={t('eq.close')}
                            onclick={close}
                        />
                    </div>
                </div>
            </div>
            <div class="flex gap-2 flex-wrap mt-4.5 mb-4">
                {#each PRESET_ORDER as name (name)}
                    <Chip
                        label={t(PRESET_LABEL_KEYS[name])}
                        active={eqStore.preset === name}
                        onclick={() => eqStore.setPreset(name as EqPresetName)}
                    />
                {/each}
            </div>
            <div class="rounded-card overflow-hidden border border-border mb-6">
                <ToggleRow
                    label={t('eq.linearPhase')}
                    description={linearPhaseDescription}
                    checked={eqStore.linearPhase}
                    onToggle={() => eqStore.setLinearPhase(!eqStore.linearPhase)}
                />
            </div>
            <div class="flex justify-between gap-1.5">
                {#each FREQ_LABELS as label, i (label)}
                    <EqBandSlider
                        value={eqStore.gains[i]}
                        freqLabel={label}
                        onInput={(v) => eqStore.setBand(i, v)}
                    />
                {/each}
            </div>
        </div>
    </div>
{/if}
