<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import Chart from '$lib/Chart.svelte';
    import {
        buildDailyLineOption,
        buildHeatmapOption,
        buildWeekBarOption,
    } from '$lib/dashboardCharts';
    import { observeTheme, type Theme } from '$lib/theme';
    import {
        listDailyPlaytime,
        onDaemonReconnected,
        onSessionEnded,
        onSessionStarted,
        type DailyPlaytime,
    } from '$lib/api';
    import { formatSeconds } from '$lib/format';

    // One fetch, 365 days. All three charts slice the same dataset —
    // the yearly span feeds the heatmap, the last 30 days feed the
    // line chart, and the current ISO week feeds the bar chart.
    let rows = $state<DailyPlaytime[]>([]);
    let loading = $state(true);
    let error = $state<string | null>(null);
    let theme = $state<Theme>('light');

    async function refresh() {
        try {
            rows = await listDailyPlaytime(365);
            error = null;
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
        }
    }

    onMount(() => {
        refresh();
        const unlistenTheme = observeTheme((t) => {
            theme = t;
        });
        const unlisteners: Promise<UnlistenFn>[] = [
            onSessionStarted(refresh),
            onSessionEnded(refresh),
            onDaemonReconnected(refresh),
        ];
        return () => {
            unlistenTheme();
            for (const p of unlisteners) {
                p.then((unlisten) => unlisten()).catch(() => {});
            }
        };
    });

    // Derived options re-compute whenever `rows` or `theme` change,
    // so the toggle button's theme swap and a signal-driven refresh
    // both flow through the same path.
    const dailyOption = $derived(buildDailyLineOption(rows, theme));
    const heatmapOption = $derived(buildHeatmapOption(rows, theme));
    const weekOption = $derived(buildWeekBarOption(rows, theme));

    // Small header stats to give the page something to read while
    // the eye skims over the charts.
    const totals = $derived.by(() => {
        let full = 0;
        let sessions = 0;
        let activeDays = 0;
        for (const r of rows) {
            full += r.full_runtime_seconds;
            sessions += r.session_count;
            if (r.session_count > 0) activeDays += 1;
        }
        return { full, sessions, activeDays };
    });
</script>

<main>
    <header>
        <h1>Dashboard</h1>
        <button onclick={refresh} disabled={loading}>Refresh</button>
    </header>

    {#if loading && rows.length === 0}
        <p class="hint">Loading…</p>
    {:else if error}
        <div class="error">
            <p><strong>Couldn't reach the daemon.</strong></p>
            <p class="detail">{error}</p>
            <p class="hint">Is <code>ludex-daemon</code> running?</p>
        </div>
    {:else}
        <section class="summary">
            <div class="stat-card">
                <div class="stat-label">Full runtime · 12 mo</div>
                <div class="stat-value">{formatSeconds(totals.full)}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Sessions · 12 mo</div>
                <div class="stat-value">{totals.sessions}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Active days</div>
                <div class="stat-value">{totals.activeDays}</div>
            </div>
        </section>

        <section>
            <h2>Daily playtime · last 30 days</h2>
            <Chart
                option={dailyOption}
                height="260px"
                label="Daily playtime over the last 30 days"
            />
        </section>

        <section>
            <h2>Activity · last 12 months</h2>
            <Chart
                option={heatmapOption}
                height="180px"
                label="Per-day activity heatmap over the last twelve months"
            />
        </section>

        <section>
            <h2>This week</h2>
            <Chart
                option={weekOption}
                height="240px"
                label="Hours played each day of the current week"
            />
        </section>
    {/if}
</main>

<style>
    main {
        max-width: 88ch;
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

    h2 {
        font-size: 1rem;
        font-weight: 600;
        color: var(--text-label);
        margin: 0 0 0.5rem;
    }

    section {
        background: var(--bg-surface);
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 1rem 1.25rem;
        margin-bottom: 1rem;
    }

    section.summary {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
        gap: 0.75rem;
        background: transparent;
        border: none;
        padding: 0;
    }

    .stat-card {
        background: var(--bg-surface);
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 0.9rem 1rem;
    }

    .stat-label {
        color: var(--text-subtle);
        font-size: 0.75rem;
        text-transform: uppercase;
        letter-spacing: 0.03em;
        margin-bottom: 0.25rem;
    }

    .stat-value {
        color: var(--text-primary);
        font-size: 1.15rem;
        font-weight: 600;
        font-variant-numeric: tabular-nums;
    }
</style>
