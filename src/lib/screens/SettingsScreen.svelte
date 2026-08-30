<script lang="ts">
    import { eqStore } from '$lib/features/eq/EqStore.svelte';
    import { PRESET_ORDER, PRESET_LABEL_KEYS, type EqPresetName } from '$lib/features/eq/presets';
    import { preferencesStore } from '$lib/features/settings/PreferencesStore.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import OpenEqButton from '$lib/features/eq/OpenEqButton.svelte';
    import { languageStore, LANGUAGES, t, type Language } from '$lib/i18n/LanguageStore.svelte';
    import type { PlaybackOption } from '$lib/models/Preferences';
    import type { TranslationKey } from '$lib/i18n/types';
    import Chip from '$lib/design-system/atoms/Chip.svelte';
    import Eyebrow from '$lib/design-system/atoms/Eyebrow.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import ToggleRow from '$lib/design-system/molecules/ToggleRow.svelte';
    import { isMac } from '$lib/utils/platform';

    const PLAYBACK_ROWS: { option: PlaybackOption; label: TranslationKey; desc: TranslationKey }[] =
        [
            { option: 'gapless', label: 'settings.gapless', desc: 'settings.gapless.desc' },
            { option: 'crossfade', label: 'settings.crossfade', desc: 'settings.crossfade.desc' },
            { option: 'normalize', label: 'settings.normalize', desc: 'settings.normalize.desc' },
            { option: 'exclusive', label: 'settings.exclusive', desc: 'settings.exclusive.desc' },
        ];

    const LANGUAGE_LABELS: Record<Language, string> = { en: 'English', fr: 'Français' };

    // Both read from the engine, which reports what the output device
    // actually opened at — populated once something has played.
    const sampleRateLabel = $derived(
        playerStore.sampleRate ? `${(playerStore.sampleRate / 1000).toFixed(1)} kHz` : '—',
    );
</script>

<div class="px-11 pt-9 pb-10 animate-float-up max-w-190">
    <Eyebrow color="violet" class="mb-2">{t('settings.eyebrow')}</Eyebrow>
    <h1 class="font-display font-black text-display tracking-tight m-0 mb-7">
        {t('settings.title')}
    </h1>

    <SectionLabel class="mb-3">{t('settings.eqPresets')}</SectionLabel>
    <div class="flex gap-2.5 flex-wrap mb-4">
        {#each PRESET_ORDER as name (name)}
            <Chip
                label={t(PRESET_LABEL_KEYS[name])}
                active={eqStore.preset === name}
                onclick={() => eqStore.setPreset(name as EqPresetName)}
            />
        {/each}
    </div>
    <div class="mb-8">
        <OpenEqButton labelKey="settings.fineTune" />
    </div>

    <SectionLabel class="mb-3">{t('settings.playback')}</SectionLabel>
    <div class="rounded-card overflow-hidden border border-border mb-8">
        {#each PLAYBACK_ROWS as row (row.option)}
            <ToggleRow
                label={t(row.label)}
                description={t(row.desc)}
                checked={preferencesStore.options[row.option]}
                onToggle={() => preferencesStore.toggle(row.option)}
            />
        {/each}
    </div>

    {#if !isMac()}
        <SectionLabel class="mb-3">{t('settings.interface')}</SectionLabel>
        <div class="rounded-card overflow-hidden border border-border mb-8">
            <ToggleRow
                label={t('settings.hideTitleBar')}
                description={t('settings.hideTitleBar.desc')}
                checked={preferencesStore.hideTitleBar}
                onToggle={() => preferencesStore.setHideTitleBar(!preferencesStore.hideTitleBar)}
            />
        </div>
    {/if}

    <SectionLabel class="mb-3">{t('settings.output')}</SectionLabel>
    <div class="flex gap-3.5 flex-wrap mb-8">
        <div class="flex-1 min-w-50 bg-panel border border-border rounded-xl px-4 py-3.5">
            <div class="text-caption text-text-muted mb-1">{t('settings.device')}</div>
            <div class="font-semibold">{playerStore.deviceName ?? t('settings.deviceValue')}</div>
        </div>
        <div class="flex-1 min-w-50 bg-panel border border-border rounded-xl px-4 py-3.5">
            <div class="text-caption text-text-muted mb-1">{t('settings.sampleRate')}</div>
            <div class="font-semibold">{sampleRateLabel}</div>
        </div>
    </div>

    <SectionLabel class="mb-3">{t('settings.language')}</SectionLabel>
    <div class="flex gap-2.5 flex-wrap mb-8">
        {#each LANGUAGES as language (language)}
            <Chip
                label={LANGUAGE_LABELS[language]}
                active={languageStore.current === language}
                onclick={() => languageStore.set(language)}
            />
        {/each}
    </div>

    <div
        class="p-4.5 rounded-card bg-banner-gradient border border-border-strong flex items-center gap-3.5"
    >
        <div class="size-9 rounded-lg bg-prism-conic flex-none"></div>
        <div class="flex-1">
            <div class="font-bold">{t('settings.banner.title')}</div>
            <div class="text-body text-text-soft">{t('settings.banner.desc')}</div>
        </div>
    </div>
</div>
