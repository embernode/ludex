<script lang="ts" module>
    /**
     * How a game was detected, keyed by the wire's `launcher_type`.
     *
     * The column this appears under asks "detected via", so `native`
     * reads as "Fallback": those rows are the ones the foreground-
     * window source picked up because no launcher claimed them.
     *
     * Colours are the design's cool source palette and are
     * scheme-independent. Note the sub-labels the design shows
     * elsewhere (`Lutris · pga.db`, `Heroic · Epic`) are not
     * reproducible — that provenance is known to the enricher but
     * never persisted — so the bare launcher name is the honest
     * rendering.
     */
    const SOURCES: Record<string, { color: string; label: string }> = {
        steam: { color: '#6d94c4', label: 'Steam' },
        heroic: { color: '#9c8ac4', label: 'Heroic' },
        lutris: { color: '#9aa2ab', label: 'Lutris' },
        flatpak: { color: '#6f767c', label: 'Flatpak' },
        native: { color: '#6f767c', label: 'Fallback' },
    };
</script>

<script lang="ts">
    interface Props {
        launcherType: string;
    }
    let { launcherType }: Props = $props();

    const source = $derived(
        SOURCES[launcherType] ?? { color: '#6f767c', label: launcherType },
    );
</script>

<span class="source">
    <span class="dot" style="background:{source.color}"></span>
    <span class="label">{source.label}</span>
</span>

<style>
    .source {
        display: flex;
        align-items: center;
        gap: 6px;
        min-width: 0;
    }

    .dot {
        width: 6px;
        height: 6px;
        border-radius: 2px;
        flex: none;
    }

    .label {
        font-size: 11.5px;
        color: var(--fg2);
        white-space: nowrap;
    }
</style>
