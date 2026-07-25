<script lang="ts">
    import AppearanceCard from '$lib/settings/AppearanceCard.svelte';
    import BackupsCard from '$lib/settings/BackupsCard.svelte';
    import DateTimeFormatCard from '$lib/settings/DateTimeFormatCard.svelte';
    import DetectionCard from '$lib/settings/DetectionCard.svelte';
    import DetectionsLinkCard from '$lib/settings/DetectionsLinkCard.svelte';
    import GraceWindowsCard from '$lib/settings/GraceWindowsCard.svelte';
    import StatusStrip from '$lib/settings/StatusStrip.svelte';

    /**
     * Per-action error sink. Each card owns its own load + save state
     * and bubbles errors up via `onerror`; the page renders a single
     * dismissable banner so multiple cards failing at once (e.g.
     * daemon down) don't stack identical messages.
     *
     * Per-field validation failures do NOT come here — they render
     * inside the row that caused them, next to the value the user
     * needs to fix.
     */
    let error = $state<string | null>(null);

    function handleError(message: string) {
        error = message;
    }
</script>

<main>
    <header>
        <h1>Settings</h1>
        <span class="apply-note">Changes apply immediately</span>
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

    <AppearanceCard />
    <DetectionCard onerror={handleError} />
    <GraceWindowsCard onerror={handleError} />
    <BackupsCard onerror={handleError} />
    <DetectionsLinkCard onerror={handleError} />
    <DateTimeFormatCard />

    <StatusStrip onerror={handleError} />
</main>

<style>
    main {
        max-width: 760px;
        margin: 0 auto;
        padding: 22px 20px 40px;
    }

    header {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 14px;
        margin-bottom: 16px;
    }

    h1 {
        font-size: 24px;
        font-weight: 600;
        line-height: 1;
        margin: 0;
        letter-spacing: -0.02em;
    }

    .apply-note {
        font-size: 11.5px;
        color: var(--fg3);
    }
</style>
