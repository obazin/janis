<script lang="ts">
    import type { Station } from '$lib/models/Station';
    import ArtTile from '$lib/design-system/atoms/ArtTile.svelte';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    interface Props {
        station: Station;
        active: boolean;
        /** True while this station is being connected and buffered. */
        connecting?: boolean;
        onclick: () => void;
    }

    let { station, active, connecting = false, onclick }: Props = $props();
</script>

<button
    class="flex gap-3.5 items-center p-3.5 rounded-card bg-panel border cursor-pointer text-left transition-colors duration-base hover:bg-panel-hover
    {active ? 'border-ember/50' : 'border-border'}"
    {onclick}
>
    <ArtTile
        seed={station.id}
        gradIndex={station.gradIndex}
        class="size-13.5 rounded-xl flex-none"
    />
    <div class="flex-1 min-w-0">
        <div class="font-bold text-heading-sm truncate">{station.name}</div>
        <div class="text-caption text-text-muted">
            {t(station.genreKey)} · {t('radio.kbps', { kbps: station.kbps })}
        </div>
    </div>
    <div
        class="flex items-center gap-1.25 text-label font-bold uppercase
        {connecting ? 'text-text-muted' : 'text-live'}"
    >
        <span
            class="size-1.75 rounded-full
            {connecting ? 'bg-text-muted animate-pulse' : 'bg-live glow-live'}"
        ></span>
        {connecting ? t('radio.connecting') : t('radio.live')}
    </div>
</button>
