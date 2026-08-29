<script lang="ts">
    import { browser } from '$app/environment';
    import Badge from '$lib/design-system/atoms/Badge.svelte';
    import { searchQuery } from '$lib/stores/SearchStore';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // App chrome header: brand + search. The window uses the macOS overlay
    // title bar, so the native traffic lights sit over the left edge — the
    // spacer keeps the brand clear of them. `data-tauri-drag-region` makes
    // the empty areas drag the window.

    const isMac = browser && navigator.userAgent.includes('Macintosh');
</script>

<header
    data-tauri-drag-region
    class="h-13 flex-none flex items-center gap-4 px-4 bg-titlebar border-b border-divider"
>
    {#if isMac}
        <div class="w-16 flex-none"></div>
    {/if}
    <div class="flex items-center gap-2.5">
        <div class="size-6 rounded-lg bg-prism-conic"></div>
        <span class="font-display font-black tracking-brand text-heading-sm">JANIS</span>
        <Badge variant="outline">{t('titlebar.openSource')}</Badge>
    </div>
    <div class="flex-1 flex justify-center" data-tauri-drag-region>
        <div
            class="flex items-center gap-2.25 w-105 max-w-2/5 bg-input-bg border border-border-strong rounded-btn px-3.25 py-2"
        >
            <svg
                class="size-3.75 text-text-muted flex-none"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
            >
                <path d={ICONS.search} />
            </svg>
            <input
                placeholder={t('titlebar.search')}
                bind:value={$searchQuery}
                class="flex-1 bg-transparent border-none outline-none text-body text-text placeholder:text-text-muted min-w-0"
            />
        </div>
    </div>
    <div class="w-16 flex-none" data-tauri-drag-region></div>
</header>
