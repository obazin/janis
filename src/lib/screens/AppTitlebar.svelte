<script lang="ts">
    import { getCurrentWindow } from '@tauri-apps/api/window';
    import Badge from '$lib/design-system/atoms/Badge.svelte';
    import IconButton from '$lib/design-system/atoms/IconButton.svelte';
    import { IconButtonIdentifier } from '$lib/design-system/atoms/IconButtonIdentifier';
    import { preferencesStore } from '$lib/features/settings/PreferencesStore.svelte';
    import { searchQuery } from '$lib/stores/SearchStore';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import { isMac } from '$lib/utils/platform';

    // App chrome header: brand + search, and — when the native frame is hidden
    // on Windows/Linux — the window controls. macOS uses the overlay title bar,
    // so the native traffic lights sit over the left edge and the spacer keeps
    // the brand clear of them. `data-tauri-drag-region` makes the non-interactive
    // areas drag the window (it sits on the brand and search areas, not the
    // control buttons, which must stay clickable).

    const macOS = isMac();
</script>

<header class="h-13 flex-none flex items-center gap-4 px-4 bg-titlebar border-b border-divider">
    {#if macOS}
        <div class="w-16 flex-none"></div>
    {/if}
    <div class="flex items-center gap-2.5" data-tauri-drag-region>
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
    {#if !macOS && preferencesStore.frameless}
        <div class="flex items-center gap-2 flex-none">
            <IconButton
                identifier={IconButtonIdentifier.WindowMinimize}
                d={ICONS.minimize}
                label={t('titlebar.minimize')}
                tone="muted"
                onclick={() => void getCurrentWindow().minimize()}
            />
            <IconButton
                identifier={IconButtonIdentifier.WindowMaximize}
                d={ICONS.maximize}
                label={t('titlebar.maximizeOrRestore')}
                tone="muted"
                onclick={() => void getCurrentWindow().toggleMaximize()}
            />
            <IconButton
                identifier={IconButtonIdentifier.WindowClose}
                d={ICONS.close}
                label={t('titlebar.close')}
                tone="muted"
                onclick={() => void getCurrentWindow().close()}
            />
        </div>
    {:else}
        <div class="w-16 flex-none" data-tauri-drag-region></div>
    {/if}
</header>
