<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import Chart from '$lib/Chart.svelte';
    import ConfirmDialog from '$lib/ConfirmDialog.svelte';
    import {
        buildHeatmapOption,
        buildRecentBarOption,
    } from '$lib/dashboardCharts';
    import { observeTheme, type Theme } from '$lib/theme';
    import { currentAccent } from '$lib/themeState.svelte';
    import { buildLanes } from '$lib/activityGrid';
    import {
        deleteSession,
        listBlockedApplicationIds,
        listDailyPlaytime,
        listRecentSessions,
        listSessionsInRange,
        onBlocklistChanged,
        onDaemonReconnected,
        onSessionEnded,
        onSessionStarted,
        type DailyPlaytime,
        type SessionSummary,
    } from '$lib/api';
    import {
        currentTimestampFormat,
        formatDate,
        formatDuration,
        formatHoursMinutes,
        formatTime,
        formatTimestamp,
        observeTimestampFormat,
        outcomeLabel,
        relativeDayName,
        sharePercent,
        type TimestampFormat,
    } from '$lib/format';

    /** Days shown in the clock grid. */
    const GRID_DAYS = 7;
    /** Sessions the log pane lists. */
    const LOG_LIMIT = 100;

    let pane = $state<'charts' | 'log'>('charts');

    let rows = $state<DailyPlaytime[]>([]);
    let weekSessions = $state<SessionSummary[]>([]);
    let recent = $state<SessionSummary[]>([]);
    /** Blocked applications are hidden here as they are in the
     *  library, and as the daemon already hides them from the daily
     *  aggregates — otherwise blocking a game would remove it from
     *  the charts while leaving it in the grid and the log. */
    let blocked = $state<Set<number>>(new Set());
    let loading = $state(true);
    let error = $state<string | null>(null);
    let theme = $state<Theme>('light');
    let tsFormat = $state<TimestampFormat>(currentTimestampFormat());

    /** Drives the grid's right edge. Ticks only while something is
     *  open, so an in-progress session's block keeps growing instead
     *  of freezing at the instant the page loaded. */
    let now = $state<number>(Date.now());

    const hasOpenSession = $derived(weekSessions.some((s) => !s.ended_at));

    $effect(() => {
        if (!hasOpenSession) return;
        const tick = setInterval(() => (now = Date.now()), 1000);
        return () => clearInterval(tick);
    });

    // `currentAccent()` is read purely to create the dependency: the
    // chart palette resolves the accent from the page's live tokens,
    // which Svelte cannot see, so without this the charts would keep
    // the previous accent until some other input changed.
    const barOption = $derived.by(() => {
        currentAccent();
        return buildRecentBarOption(rows, theme, tsFormat);
    });
    const heatmapOption = $derived.by(() => {
        currentAccent();
        return buildHeatmapOption(rows, theme, tsFormat);
    });

    const lanes = $derived(buildLanes(weekSessions, GRID_DAYS, now));

    const DAY_LABELS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

    /** Accent percentages sampling the heatmap's colour ramp for its
     *  less→more key. See the legend markup for why these five. */
    const HEAT_STOPS = [0, 17, 34, 67, 100];

    /** Totals across the fetched year, shown on the consistency card. */
    const yearTotals = $derived.by(() => {
        let full = 0;
        let sessions = 0;
        let activeDays = 0;
        for (const r of rows) {
            full += r.full_runtime_seconds;
            sessions += r.session_count;
            if (r.full_runtime_seconds > 0) activeDays += 1;
        }
        return { full, sessions, activeDays };
    });

    /** Log rows grouped into days, newest day first. */
    const logDays = $derived.by(() => {
        const groups = new Map<string, SessionSummary[]>();
        for (const s of recent) {
            const started = new Date(s.started_at);
            if (Number.isNaN(started.getTime())) continue;
            // Local calendar day, matching how the daemon buckets.
            const key = `${started.getFullYear()}-${String(
                started.getMonth() + 1,
            ).padStart(2, '0')}-${String(started.getDate()).padStart(2, '0')}`;
            const bucket = groups.get(key);
            if (bucket) bucket.push(s);
            else groups.set(key, [s]);
        }
        return [...groups.entries()].map(([date, items]) => ({
            date,
            // Rendered from the local date parts the key was built
            // from. Re-parsing the key as an ISO instant would shift
            // the heading a day west of UTC, labelling every group
            // with the previous date.
            name: relativeDayName(items[0].started_at),
            label: dayHeading(items[0].started_at),
            items,
            total: items.reduce((n, s) => n + s.full_runtime_seconds, 0),
        }));
    });

    /**
     * Calendar date for a day heading, carried beside the day's name.
     * Deliberately not a timestamp — a day group has no time of day.
     *
     * Forced to an absolute format: `relativeDayName` already supplies
     * the relative half, so honouring a `relative` preference here
     * would print "Today" twice.
     */
    function dayHeading(startedAt: string): string {
        return formatDate(
            startedAt,
            tsFormat === 'relative' ? 'short' : tsFormat,
        );
    }

    function laneLabel(dayStartMs: number): { day: string; dom: string } {
        const d = new Date(dayStartMs);
        return {
            day: DAY_LABELS[d.getDay()],
            dom: String(d.getDate()).padStart(2, '0'),
        };
    }

    /** Merged-fragment count, carried under the game name rather than
     *  in the outcome column — it describes the row's composition,
     *  not how the session ended. */
    function mergedNote(s: SessionSummary): string {
        return s.fragment_ids.length > 1
            ? `· ${s.fragment_ids.length} merged`
            : '';
    }

    /**
     * Whether a log row is safe to delete from here. Open sessions
     * belong to the daemon and would lose in-flight runtime, so they
     * offer no button — the same rule the game-detail list applies.
     * Merged spans are fine: the daemon is handed the span's
     * `fragment_ids` and drops exactly those rows in one transaction.
     */
    function canDelete(s: SessionSummary): boolean {
        return Boolean(s.exit_reason);
    }

    /** Underlying `<dialog>`, owned by `ConfirmDialog` but bound back
     *  here so it can be opened and closed imperatively. */
    let deleteDialog = $state<HTMLDialogElement | null>(null);
    /** Session queued for deletion while the dialog is open. */
    let pendingDelete = $state<SessionSummary | null>(null);

    function openDeleteDialog(s: SessionSummary) {
        pendingDelete = s;
        deleteDialog?.showModal();
    }

    async function performDelete() {
        if (!pendingDelete) return;
        try {
            await deleteSession(pendingDelete.fragment_ids);
            deleteDialog?.close();
            pendingDelete = null;
            // A full refresh, not a local splice: removing a session
            // moves the day group's total, the charts and the clock
            // grid, all of which are derived from separate fetches.
            await refresh();
        } catch (e) {
            error = String(e);
            // Re-thrown so the dialog clears its busy state and stays
            // open, with the error banner visible above it.
            throw e;
        }
    }

    async function refresh() {
        try {
            now = Date.now();
            const gridStart = new Date(now);
            gridStart.setHours(0, 0, 0, 0);
            gridStart.setDate(gridStart.getDate() - (GRID_DAYS - 1));

            const [daily, week, log, blockedIds] = await Promise.all([
                listDailyPlaytime(365),
                // The upper bound is exclusive, so nudge it past the
                // present or a session starting this very millisecond
                // would fall outside its own window.
                listSessionsInRange(
                    gridStart.toISOString(),
                    new Date(now + 1000).toISOString(),
                ),
                listRecentSessions(LOG_LIMIT),
                listBlockedApplicationIds().catch(() => [] as number[]),
            ]);
            rows = daily;
            blocked = new Set(blockedIds);
            weekSessions = week.filter((s) => !blocked.has(s.application_id));
            recent = log.filter((s) => !blocked.has(s.application_id));
            error = null;
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
        }
    }

    onMount(() => {
        refresh();
        const unlistenTheme = observeTheme((t) => (theme = t));
        const unlistenTs = observeTimestampFormat((f) => (tsFormat = f));
        const unlisteners: Promise<UnlistenFn>[] = [
            onSessionStarted(refresh),
            onSessionEnded(refresh),
            onDaemonReconnected(refresh),
            onBlocklistChanged(refresh),
        ];
        return () => {
            unlistenTheme();
            unlistenTs();
            for (const p of unlisteners) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<main>
    <div class="titlerow">
        <h1>Activity</h1>
        <span class="subcount">
            {pane === 'charts' ? 'last 30 days' : `last ${LOG_LIMIT} sessions`}
        </span>
        <div class="spacer"></div>
        <div class="seg">
            <button
                type="button"
                aria-pressed={pane === 'log'}
                onclick={() => (pane = 'log')}
            >
                Log
            </button>
            <button
                type="button"
                aria-pressed={pane === 'charts'}
                onclick={() => (pane = 'charts')}
            >
                Charts
            </button>
        </div>
    </div>

    {#if loading && rows.length === 0}
        <p class="state">Loading…</p>
    {:else if error}
        <div class="error">
            <p><strong>Couldn't reach the daemon.</strong></p>
            <p class="detail">{error}</p>
            <p class="hint">Is <code>ludex-daemon</code> running?</p>
        </div>
    {:else if pane === 'charts'}
        <section class="card">
            <div class="cardhead">
                <span class="cardtitle">When you played</span>
                <span class="dim">last {GRID_DAYS} days</span>
                <div class="spacer"></div>
                <span class="legend">
                    <span class="swatch ac"></span>played
                </span>
            </div>
            <div class="axisrow">
                <span></span>
                <div class="axis">
                    {#each [0, 4, 8, 12, 16, 20, 24] as h (h)}
                        <span class="tick mono" style="left:{(h / 24) * 100}%">
                            {String(h).padStart(2, '0')}
                        </span>
                    {/each}
                </div>
                <span class="right collabel">TOTAL</span>
            </div>
            {#each lanes as lane (lane.dayStartMs)}
                {@const label = laneLabel(lane.dayStartMs)}
                <div class="lanerow">
                    <div class="laneday">
                        <span class="dayname">{label.day}</span>
                        <span class="mono dim">{label.dom}</span>
                    </div>
                    <div class="lane">
                        {#each [4, 8, 12, 16, 20] as h (h)}
                            <span
                                class="gridline"
                                style="left:{(h / 24) * 100}%"
                            ></span>
                        {/each}
                        {#each lane.blocks as block, i (i)}
                            <span
                                class="blk"
                                style="left:{block.leftPct}%;width:{block.widthPct}%"
                            ></span>
                        {/each}
                    </div>
                    <span class="right mono dim">
                        {lane.totalSeconds > 0
                            ? formatHoursMinutes(lane.totalSeconds)
                            : '—'}
                    </span>
                </div>
            {/each}
        </section>

        <section class="card">
            <div class="cardhead">
                <span class="cardtitle">Daily playtime</span>
                <span class="dim">last 30 days</span>
            </div>
            <Chart
                option={barOption}
                height="220px"
                label="Daily full runtime over the last 30 days"
            />
        </section>

        <section class="card">
            <div class="cardhead">
                <span class="cardtitle">Play history</span>
                <span class="dim">
                    last 12 months ·
                    <b>{formatDuration(yearTotals.full)} full</b>
                    · <b>{yearTotals.sessions} sessions</b>
                    · <b>{yearTotals.activeDays} active days</b>
                </span>
                <div class="spacer"></div>
                <!-- Samples the same ramp the heatmap paints with:
                     `visualMap` interpolates lane → 34% accent → accent
                     across the range, so these stops are that curve at
                     0 / ¼ / ½ / ¾ / 1. Expressed as `color-mix` on
                     `var(--ac)` rather than resolved values, so the key
                     tracks the accent picker like the chart does. -->
                <span class="heatlegend">
                    less
                    {#each HEAT_STOPS as pct (pct)}
                        <span
                            class="hcell"
                            style="background:color-mix(in oklab, var(--ac) {pct}%, var(--lane))"
                        ></span>
                    {/each}
                    more
                </span>
            </div>
            <Chart
                option={heatmapOption}
                height="150px"
                label="Daily activity over the last 12 months"
            />
        </section>
    {:else if recent.length === 0}
        <p class="state">No sessions recorded yet.</p>
    {:else}
        {#each logDays as day (day.date)}
            <div class="daygroup">
                <div class="dayhead">
                    <span class="daytitle">{day.name}</span>
                    <span class="daydate">{day.label}</span>
                    <div class="spacer"></div>
                    <span class="num daytotal">
                        {formatHoursMinutes(day.total)}
                    </span>
                </div>
                <ul class="rows">
                    {#each day.items as s (s.id)}
                        <li class="logrow">
                            <span class="namecell">
                                <a class="gname" href="/app/{s.application_id}">
                                    {s.product_name}
                                </a>
                                {#if mergedNote(s)}
                                    <span class="merged">{mergedNote(s)}</span>
                                {/if}
                            </span>
                            <span class="mono num dim">
                                {formatTime(s.started_at, tsFormat)} – {formatTime(
                                    s.ended_at,
                                    tsFormat,
                                )}
                            </span>
                            <span class="right num strong">
                                {formatDuration(s.full_runtime_seconds)}
                            </span>
                            <!-- This session's share of the day, not its
                                 interactive ratio. Per-session idle
                                 *intervals* aren't stored, so an
                                 interactive bar here could only restate a
                                 number the row no longer shows; against
                                 the day's total it reads directly off the
                                 group heading and the bars in a group sum
                                 to the whole day. -->
                            <span
                                class="interactive"
                                title="{Math.round(
                                    sharePercent(
                                        s.full_runtime_seconds,
                                        day.total,
                                    ),
                                )}% of {day.name.toLowerCase()}"
                            >
                                <span class="bar">
                                    <span
                                        style="width:{sharePercent(
                                            s.full_runtime_seconds,
                                            day.total,
                                        )}%"
                                    ></span>
                                </span>
                            </span>
                            <span class="status" class:open={!s.exit_reason}>
                                {outcomeLabel(s.exit_reason)}
                            </span>
                            <span class="rowaction">
                                {#if canDelete(s)}
                                    <button
                                        type="button"
                                        class="delete"
                                        title="Delete this session"
                                        aria-label="Delete the {s.product_name} session starting {formatTimestamp(
                                            s.started_at,
                                            tsFormat,
                                        )}"
                                        onclick={() => openDeleteDialog(s)}
                                    >
                                        ✕
                                    </button>
                                {/if}
                            </span>
                        </li>
                    {/each}
                </ul>
            </div>
        {/each}
    {/if}

    <ConfirmDialog
        bind:dialog={deleteDialog}
        title="Delete this session?"
        confirmLabel="Delete session"
        confirmBusyLabel="Deleting…"
        danger
        onconfirm={performDelete}
    >
        {#snippet body()}
            {#if pendingDelete}
                <dl class="confirm-facts">
                    <dt>Game</dt>
                    <dd>{pendingDelete.product_name}</dd>
                    <dt>Started</dt>
                    <dd>{formatTimestamp(pendingDelete.started_at, tsFormat)}</dd>
                    <dt>Ended</dt>
                    <dd>{formatTimestamp(pendingDelete.ended_at, tsFormat)}</dd>
                    <dt>Full</dt>
                    <dd>{formatDuration(pendingDelete.full_runtime_seconds)}</dd>
                    <dt>Interactive</dt>
                    <dd>
                        {formatDuration(pendingDelete.interactive_runtime_seconds)}
                    </dd>
                </dl>
                {#if pendingDelete.fragment_ids.length > 1}
                    <p class="confirm-warning">
                        This row is a merged span;
                        <strong>
                            all {pendingDelete.fragment_ids.length} underlying sessions
                        </strong>
                        will be removed.
                    </p>
                {/if}
                <p class="confirm-warning">
                    This cannot be undone. Aggregate stats for
                    <strong>{pendingDelete.product_name}</strong> are recomputed
                    from the surviving sessions.
                </p>
            {/if}
        {/snippet}
    </ConfirmDialog>
</main>

<style>
    main {
        max-width: 1000px;
        margin: 0 auto;
        padding: 22px 20px 40px;
    }

    .titlerow {
        display: flex;
        align-items: flex-end;
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

    .subcount {
        font-size: 13px;
        padding-bottom: 2px;
        color: var(--fg3);
    }

    .spacer {
        flex: 1;
    }

    .seg {
        display: flex;
        gap: 3px;
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 7px;
        padding: 3px;
    }

    .seg button {
        font-size: 12px;
        font-weight: 500;
        border-radius: 5px;
        padding: 4px 12px;
        border: 0;
        background: transparent;
        color: var(--fg2);
        cursor: pointer;
    }

    .seg button[aria-pressed='true'] {
        background: var(--ac);
        color: var(--bg);
    }

    .card {
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 9px;
        padding: 14px 16px 12px;
        margin-bottom: 14px;
    }

    .cardhead {
        display: flex;
        align-items: baseline;
        gap: 10px;
        margin-bottom: 12px;
        flex-wrap: wrap;
    }

    .cardtitle {
        font-size: 13.5px;
        font-weight: 600;
        color: var(--fg);
    }

    .dim {
        font-size: 11.5px;
        color: var(--fg3);
    }

    .cardhead b {
        color: var(--fg2);
        font-weight: 600;
    }

    .legend {
        display: flex;
        align-items: center;
        gap: 5px;
        font-size: 11px;
        color: var(--fg2);
    }

    .swatch {
        width: 9px;
        height: 9px;
        border-radius: 2px;
    }

    .swatch.ac {
        background: var(--ac);
    }

    .heatlegend {
        display: flex;
        align-items: center;
        gap: 4px;
        font-size: 11px;
        color: var(--fg3);
    }

    .hcell {
        width: 9px;
        height: 9px;
        border-radius: 2px;
    }

    .axisrow,
    .lanerow {
        display: grid;
        grid-template-columns: 74px 1fr 62px;
        gap: 12px;
        align-items: center;
    }

    .axisrow {
        margin-bottom: 6px;
    }

    .axis {
        position: relative;
        height: 13px;
    }

    .tick {
        position: absolute;
        top: 0;
        font-size: 9.5px;
        transform: translateX(-50%);
        color: var(--fg3);
    }

    .collabel {
        font-size: 10px;
        font-weight: 500;
        letter-spacing: 0.08em;
        color: var(--fg3);
    }

    .lanerow {
        padding: 2px 0;
    }

    .laneday {
        display: flex;
        align-items: baseline;
        gap: 6px;
    }

    .dayname {
        font-size: 12px;
        font-weight: 500;
        color: var(--fg2);
    }

    .lane {
        position: relative;
        height: 20px;
        border-radius: 4px;
        overflow: hidden;
        background: var(--lane);
    }

    .gridline {
        position: absolute;
        top: 0;
        bottom: 0;
        width: 1px;
        background: var(--lane-line);
    }

    .blk {
        position: absolute;
        top: 4px;
        bottom: 4px;
        border-radius: 3px;
        min-width: 3px;
        background: var(--ac);
    }

    .daygroup {
        margin-bottom: 18px;
    }

    .dayhead {
        display: flex;
        align-items: baseline;
        gap: 10px;
        padding-bottom: 7px;
        border-bottom: 1px solid var(--line);
        margin-bottom: 2px;
    }

    /* Two weights, as the design draws it: the day reads first and the
       date qualifies it. One bold string carrying both scanned as a
       wall of text down a page of groups. */
    .daytitle {
        font-size: 12.5px;
        font-weight: 600;
        color: var(--fg);
    }

    .daydate {
        font-size: 11.5px;
        color: var(--fg3);
    }

    .daytotal {
        font-size: 12px;
        font-weight: 500;
        color: var(--fg2);
    }

    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
    }

    .logrow {
        display: grid;
        grid-template-columns:
            minmax(160px, 1fr) 132px 78px 150px minmax(120px, 1fr) 30px;
        gap: 12px;
        align-items: center;
        padding: 9px 12px;
        border-bottom: 1px solid var(--hair);
    }

    .namecell {
        min-width: 0;
    }

    .gname {
        display: block;
        font-size: 13px;
        font-weight: 500;
        color: var(--fg);
        text-decoration: none;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .merged {
        display: block;
        font-size: 11px;
        color: var(--fg3);
    }

    .gname:hover {
        text-decoration: underline;
    }

    .right {
        text-align: right;
    }

    .num {
        font-variant-numeric: tabular-nums;
    }

    .mono {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 12px;
    }

    .strong {
        font-size: 12.5px;
        font-weight: 500;
    }

    .interactive {
        display: flex;
        align-items: center;
        gap: 9px;
        min-width: 0;
    }

    .bar {
        flex: 1;
        height: 5px;
        border-radius: 99px;
        overflow: hidden;
        background: var(--track);
    }

    .bar > span {
        display: block;
        height: 100%;
        background: var(--ac);
    }

    .status {
        font-size: 11.5px;
        color: var(--fg2);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .status.open {
        color: var(--ac);
    }

    .state {
        font-size: 12.5px;
        color: var(--fg3);
        padding: 20px 12px;
        margin: 0;
    }

    .rowaction {
        text-align: center;
    }

    .delete {
        font: inherit;
        font-size: 12px;
        color: var(--fg3);
        background: none;
        border: 0;
        padding: 2px 4px;
        cursor: pointer;
        border-radius: 4px;
    }

    .delete:focus-visible {
        outline: 2px solid var(--ac);
        outline-offset: -1px;
    }

    .delete:hover {
        color: var(--warn);
        background: none;
    }

    /* Body content rendered into the ConfirmDialog component's `body`
       snippet. The component owns the frame / buttons / ::backdrop;
       the parent supplies the content and its styling. */
    .confirm-facts {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 0.3rem 1rem;
        margin: 0 0 1rem;
        font-size: 0.85rem;
    }

    .confirm-facts dt {
        color: var(--fg3);
    }

    .confirm-facts dd {
        margin: 0;
        color: var(--fg2);
    }

    .confirm-warning {
        font-size: 0.85rem;
        color: var(--fg2);
        margin: 0 0 0.5rem;
        line-height: 1.5;
    }
</style>
