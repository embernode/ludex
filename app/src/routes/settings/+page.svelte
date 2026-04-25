<script lang="ts">
    import AboutCard from '$lib/settings/AboutCard.svelte';
    import AltTabCard from '$lib/settings/AltTabCard.svelte';
    import BackupsCard from '$lib/settings/BackupsCard.svelte';
    import BlockedGamesCard from '$lib/settings/BlockedGamesCard.svelte';
    import DateTimeFormatCard from '$lib/settings/DateTimeFormatCard.svelte';
    import DetectionThresholdsCard from '$lib/settings/DetectionThresholdsCard.svelte';

    /**
     * Per-action error sink. Each card owns its own load + save
     * state and bubbles errors up via `onerror`; the page renders
     * a single dismissable banner so multiple cards failing at
     * once (e.g. daemon down) don't stack six identical messages.
     */
    let error = $state<string | null>(null);

    function handleError(message: string) {
        error = message;
    }
</script>

<main>
    <header>
        <h1>Settings</h1>
    </header>

    {#if error}
        <div class="error inline">
            <p class="detail">{error}</p>
            <button
                type="button"
                class="link-button"
                onclick={() => (error = null)}
                aria-label="Dismiss"
            >
                Dismiss
            </button>
        </div>
    {/if}

    <DetectionThresholdsCard onerror={handleError} />
    <AltTabCard onerror={handleError} />
    <DateTimeFormatCard />
    <BackupsCard onerror={handleError} />
    <BlockedGamesCard onerror={handleError} />
    <AboutCard onerror={handleError} />
</main>

<style>
    main {
        max-width: 80ch;
        margin: 0 auto;
        padding: 2rem;
    }

    header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 1.5rem;
    }

    h1 {
        font-size: 1.75rem;
        font-weight: 600;
        margin: 0;
        letter-spacing: -0.02em;
    }
</style>
