<script lang="ts">
    // App-shell error toasts. Mounted once in the layout so a failure surfaces
    // on any screen. Click a toast to dismiss it; each also auto-dismisses.
    import { notificationStore } from '$lib/stores/NotificationStore.svelte';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import { ICONS } from '$lib/icons/paths';
</script>

{#if notificationStore.items.length}
    <div
        class="fixed bottom-24 right-6 z-overlay flex flex-col gap-2 w-full max-w-96"
        role="region"
        aria-live="assertive"
    >
        {#each notificationStore.items as item (item.id)}
            <button
                type="button"
                class="flex items-start gap-2.5 w-full text-left cursor-pointer rounded-card border border-error/40 bg-miniplayer backdrop-blur-xl px-4 py-3 shadow-tile animate-float-up"
                title={t('notification.dismiss')}
                onclick={() => notificationStore.dismiss(item.id)}
            >
                <svg
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.6"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="size-4.5 flex-none text-error mt-0.5"
                    aria-hidden="true"
                >
                    <path d={ICONS.alert} />
                </svg>
                <span class="flex-1 text-body text-text">{t(item.messageKey, item.params)}</span>
                <svg
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.6"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="size-3.5 flex-none text-text-muted mt-0.5"
                    aria-hidden="true"
                >
                    <path d={ICONS.close} />
                </svg>
            </button>
        {/each}
    </div>
{/if}
