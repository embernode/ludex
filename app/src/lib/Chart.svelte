<script lang="ts">
    import { onMount } from 'svelte';
    import { echarts, type EChartsCoreOption } from './echartsSetup';

    interface Props {
        option: EChartsCoreOption;
        height?: string;
        /**
         * Optional aria-label for the wrapping div. The chart
         * itself is a `<canvas>` with no intrinsic semantics; this
         * is how screen readers are told what the visual is.
         */
        label?: string;
    }

    let { option, height = '280px', label }: Props = $props();

    let container: HTMLDivElement;
    let chart: ReturnType<typeof echarts.init> | null = null;

    onMount(() => {
        // `renderer: 'canvas'` is the registered default; make it
        // explicit so a future package upgrade that flips defaults
        // doesn't silently change our output.
        chart = echarts.init(container, null, { renderer: 'canvas' });
        chart.setOption(option);

        // Webview resizes don't fire `window.resize` reliably
        // inside Tauri. Observe the element instead so the chart
        // re-lays out when the route's container changes.
        const ro = new ResizeObserver(() => chart?.resize());
        ro.observe(container);

        return () => {
            ro.disconnect();
            chart?.dispose();
            chart = null;
        };
    });

    // Replace-mode (`true`) so theme/palette swaps wipe the
    // previous axis, tooltip, and series definitions instead of
    // merging — merge mode keeps stale colors around when the
    // option is a fresh object from a different palette.
    $effect(() => {
        chart?.setOption(option, true);
    });
</script>

<div
    bind:this={container}
    class="chart"
    style="height: {height};"
    role={label ? 'img' : undefined}
    aria-label={label}
></div>

<style>
    .chart {
        width: 100%;
    }
</style>
