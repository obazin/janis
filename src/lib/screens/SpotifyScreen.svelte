<script lang="ts">
    import PrimaryButton from '$lib/design-system/atoms/PrimaryButton.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // Connect screen. The integration itself is on the roadmap — the CTA is
    // honest about that instead of pretending to link an account.
    let showNote = $state(false);
</script>

<div class="px-11 pt-9 pb-10 animate-float-up">
    <div class="max-w-140 mx-auto my-10 text-center">
        <div
            class="size-20 rounded-full bg-spotify mx-auto mb-6 flex items-center justify-center shadow-lg"
        >
            <svg
                class="size-11 text-on-accent"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
            >
                <path d={ICONS.spotifyFull} />
            </svg>
        </div>
        <h1 class="font-display font-black text-display-sm m-0 mb-3">{t('spotify.title')}</h1>
        <p class="text-heading-sm text-text-soft leading-relaxed m-0 mb-6.5">
            {t('spotify.desc')}
        </p>
        <PrimaryButton variant="spotify" onclick={() => (showNote = true)}>
            {t('spotify.cta')}
        </PrimaryButton>
        {#if showNote}
            <div class="text-body-em text-warning mt-4">{t('spotify.comingSoon')}</div>
        {/if}
        <div class="flex gap-6.5 justify-center mt-8.5 text-body text-text-muted">
            <div>✓ {t('spotify.feature.playlists')}</div>
            <div>✓ {t('spotify.feature.search')}</div>
            <div>✓ {t('spotify.feature.eq')}</div>
        </div>
    </div>
</div>
