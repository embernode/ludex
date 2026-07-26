<script lang="ts" module>
    /**
     * How a game was detected, keyed by the wire's `launcher_type`.
     *
     * The column this appears under asks "detected via", so `native`
     * reads as "Fallback": those rows are the ones the foreground-
     * window source picked up because no launcher claimed them.
     *
     * Colours are the design's cool source palette and are
     * scheme-independent.
     */
    const SOURCES: Record<string, { color: string; label: string }> = {
        steam: { color: '#6d94c4', label: 'Steam' },
        heroic: { color: '#9c8ac4', label: 'Heroic' },
        lutris: { color: '#9aa2ab', label: 'Lutris' },
        flatpak: { color: '#6f767c', label: 'Flatpak' },
        native: { color: '#6f767c', label: 'Fallback' },
    };

    /**
     * Which enrichment source named the game, keyed by the wire's
     * `detected_via`. Shown as a sub-label under the launcher, because
     * the two answer different questions and often disagree: a Lutris
     * game named from a `.desktop` entry means the `pga.db` lookup
     * missed, which is exactly what one wants to know when a title
     * looks wrong.
     *
     * An unrecognised value renders as itself rather than vanishing, so
     * a daemon that grows a source degrades on an older GUI instead of
     * hiding the row's provenance.
     */
    const DETECTED_VIA: Record<string, string> = {
        steam: 'appmanifest',
        lutris: 'pga.db',
        heroic: 'store cache',
        gog: 'goggame.info',
        desktop: '.desktop',
        pe: 'PE header',
    };
</script>

<script lang="ts">
    interface Props {
        launcherType: string;
        /** Wire `detected_via`; empty when no source recorded one. */
        detectedVia?: string;
    }
    let { launcherType, detectedVia = '' }: Props = $props();

    const source = $derived(
        SOURCES[launcherType] ?? { color: '#6f767c', label: launcherType },
    );
    const via = $derived(detectedVia ? (DETECTED_VIA[detectedVia] ?? detectedVia) : '');
</script>

<span class="source">
    <span class="dot" style="background:{source.color}"></span>
    <span class="stack">
        <span class="label">{source.label}</span>
        {#if via}
            <span class="via">{via}</span>
        {/if}
    </span>
</span>

<style>
    .source {
        display: flex;
        align-items: baseline;
        gap: 6px;
        min-width: 0;
    }

    .dot {
        width: 6px;
        height: 6px;
        border-radius: 2px;
        flex: none;
    }

    .stack {
        min-width: 0;
    }

    .label {
        display: block;
        font-size: 11.5px;
        color: var(--fg2);
        white-space: nowrap;
    }

    /* Second line rather than an inline `Lutris · pga.db`: the row is
       already tight, and stacking keeps the launcher scannable down
       the column while the provenance stays available to anyone who
       looks. */
    .via {
        display: block;
        font-size: 10.5px;
        color: var(--fg3);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
</style>
