// Pure option builders for the three dashboard charts.
//
// Each function takes the raw DailyPlaytime rows the daemon returns
// and produces a fully-formed ECharts option object. No chart
// instance, no DOM, no theme-reactivity plumbing in here — those
// are the caller's concern. Keeping these pure makes them easy to
// spot-check with a JSON dump and keeps the Svelte page short.

import type { EChartsCoreOption } from './echartsSetup';
import type { DailyPlaytime } from './api';
import { palette } from './chartPalette';
import type { Theme } from './theme';

/** Seconds → hours with one decimal (tooltip precision). */
function hoursOf(seconds: number): number {
    return Math.round((seconds / 3600) * 10) / 10;
}

/** Humanised duration label, e.g. `2h 35m` / `45m` / `—`. */
function formatDuration(seconds: number): string {
    if (seconds <= 0) return '—';
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    if (h === 0) return `${m}m`;
    if (m === 0) return `${h}h`;
    return `${h}h ${m}m`;
}

/**
 * Shift a `YYYY-MM-DD` date string by N days. Operates in UTC so
 * the daemon's UTC-bucketed dates line up with our axis labels
 * without timezone surprises.
 */
function shiftDate(iso: string, deltaDays: number): string {
    const d = new Date(`${iso}T00:00:00Z`);
    d.setUTCDate(d.getUTCDate() + deltaDays);
    return d.toISOString().slice(0, 10);
}

/** Today (UTC) as `YYYY-MM-DD`. */
function today(): string {
    return new Date().toISOString().slice(0, 10);
}

/** Index DailyPlaytime rows by their date for O(1) lookups. */
function indexByDate(rows: readonly DailyPlaytime[]): Map<string, DailyPlaytime> {
    const m = new Map<string, DailyPlaytime>();
    for (const r of rows) m.set(r.date, r);
    return m;
}

/**
 * Line chart: last 30 days of full runtime (hours) with zero-filled
 * gaps so the axis is continuous. The tooltip shows the exact HH MM
 * figure plus interactive time and session count.
 */
export function buildDailyLineOption(
    rows: readonly DailyPlaytime[],
    theme: Theme,
): EChartsCoreOption {
    const p = palette(theme);
    const byDate = indexByDate(rows);
    const end = today();

    const days: string[] = [];
    const full: number[] = [];
    const interactive: number[] = [];
    const sessions: number[] = [];
    for (let i = 29; i >= 0; i--) {
        const d = shiftDate(end, -i);
        days.push(d);
        const row = byDate.get(d);
        full.push(hoursOf(row?.full_runtime_seconds ?? 0));
        interactive.push(hoursOf(row?.interactive_runtime_seconds ?? 0));
        sessions.push(row?.session_count ?? 0);
    }

    return {
        backgroundColor: 'transparent',
        grid: { left: 44, right: 16, top: 16, bottom: 36 },
        tooltip: {
            trigger: 'axis',
            backgroundColor: p.tooltipBg,
            borderColor: p.tooltipBorder,
            textStyle: { color: p.tooltipText },
            formatter: (params: unknown) => {
                const arr = params as Array<{ dataIndex: number; axisValue: string }>;
                const idx = arr[0]?.dataIndex ?? 0;
                const axisValue = arr[0]?.axisValue ?? '';
                const f = rowSeconds(byDate, axisValue, 'full');
                const i = rowSeconds(byDate, axisValue, 'interactive');
                const c = sessions[idx] ?? 0;
                return `<div style="font-weight:600">${axisValue}</div>
<div>Full: ${formatDuration(f)}</div>
<div>Interactive: ${formatDuration(i)}</div>
<div>Sessions: ${c}</div>`;
            },
        },
        xAxis: {
            type: 'category',
            data: days,
            axisLine: { lineStyle: { color: p.axis } },
            axisLabel: {
                color: p.axisLabel,
                formatter: (value: string) => value.slice(5), // MM-DD
            },
            axisTick: { show: false },
        },
        yAxis: {
            type: 'value',
            name: 'hours',
            nameTextStyle: { color: p.axisLabel, padding: [0, 0, 0, 32] },
            axisLabel: { color: p.axisLabel },
            axisLine: { show: false },
            splitLine: { lineStyle: { color: p.splitLine } },
        },
        series: [
            {
                name: 'Full',
                type: 'line',
                smooth: true,
                showSymbol: false,
                areaStyle: { opacity: 0.15 },
                lineStyle: { width: 2 },
                itemStyle: { color: p.series[0] },
                data: full,
            },
            {
                name: 'Interactive',
                type: 'line',
                smooth: true,
                showSymbol: false,
                lineStyle: { width: 1.5, type: 'dashed' },
                itemStyle: { color: p.series[1] },
                data: interactive,
            },
        ],
    };
}

function rowSeconds(
    byDate: Map<string, DailyPlaytime>,
    date: string,
    field: 'full' | 'interactive',
): number {
    const row = byDate.get(date);
    if (!row) return 0;
    return field === 'full' ? row.full_runtime_seconds : row.interactive_runtime_seconds;
}

/**
 * Calendar heatmap: one cell per day across roughly the last year.
 * Intensity encodes full runtime. Zero-play days render in the
 * empty-cell colour so gaps stay readable.
 */
export function buildHeatmapOption(
    rows: readonly DailyPlaytime[],
    theme: Theme,
): EChartsCoreOption {
    const p = palette(theme);
    const byDate = indexByDate(rows);
    const end = today();
    const start = shiftDate(end, -364);

    // Emit a data point for every day in the range so `visualMap`
    // colours empty days with the low end of the gradient rather
    // than falling back to the chart background.
    const data: Array<[string, number]> = [];
    let max = 0;
    for (let i = 364; i >= 0; i--) {
        const d = shiftDate(end, -i);
        const row = byDate.get(d);
        const hours = hoursOf(row?.full_runtime_seconds ?? 0);
        data.push([d, hours]);
        if (hours > max) max = hours;
    }

    return {
        backgroundColor: 'transparent',
        tooltip: {
            backgroundColor: p.tooltipBg,
            borderColor: p.tooltipBorder,
            textStyle: { color: p.tooltipText },
            formatter: (params: unknown) => {
                const p2 = params as { value: [string, number] };
                const [date, hours] = p2.value;
                const row = byDate.get(date);
                const full = row?.full_runtime_seconds ?? 0;
                const sessions = row?.session_count ?? 0;
                return `<div style="font-weight:600">${date}</div>
<div>${formatDuration(full)} · ${sessions} session${sessions === 1 ? '' : 's'}</div>
<div style="opacity:0.7">(${hours.toFixed(1)} h)</div>`;
            },
        },
        visualMap: {
            show: false,
            min: 0,
            max: Math.max(1, max),
            inRange: { color: [p.heatmapEmpty, ...p.heatmapRange] },
        },
        calendar: {
            top: 30,
            left: 30,
            right: 16,
            cellSize: ['auto', 14],
            range: [start, end],
            itemStyle: {
                color: p.heatmapEmpty,
                borderColor: p.heatmapCellBorder,
                borderWidth: 2,
            },
            splitLine: { show: false },
            yearLabel: { show: false },
            monthLabel: { color: p.axisLabel, fontSize: 11 },
            dayLabel: { color: p.axisLabel, fontSize: 10, firstDay: 1 },
        },
        series: [
            {
                type: 'heatmap',
                coordinateSystem: 'calendar',
                data,
            },
        ],
    };
}

/**
 * Bar chart: hours played each day of the current (local) week,
 * Monday through Sunday. "Current week" is evaluated once per
 * render so the chart rolls over naturally when the daemon emits a
 * new session.
 */
export function buildWeekBarOption(
    rows: readonly DailyPlaytime[],
    theme: Theme,
): EChartsCoreOption {
    const p = palette(theme);
    const byDate = indexByDate(rows);

    // Find Monday of the current UTC week. Consistent with the
    // daemon's UTC bucketing; a user in a far-east timezone who
    // plays after local midnight will see that session on the
    // "previous" UTC day — acceptable for a first pass, revisit
    // when the daemon grows a timezone-aware endpoint.
    const todayStr = today();
    const d = new Date(`${todayStr}T00:00:00Z`);
    const weekday = d.getUTCDay(); // Sun=0..Sat=6
    const deltaToMonday = weekday === 0 ? -6 : 1 - weekday;
    const monday = shiftDate(todayStr, deltaToMonday);

    const labels = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
    const hours: number[] = [];
    const dates: string[] = [];
    for (let i = 0; i < 7; i++) {
        const date = shiftDate(monday, i);
        dates.push(date);
        hours.push(hoursOf(byDate.get(date)?.full_runtime_seconds ?? 0));
    }

    return {
        backgroundColor: 'transparent',
        grid: { left: 44, right: 16, top: 16, bottom: 28 },
        tooltip: {
            trigger: 'axis',
            backgroundColor: p.tooltipBg,
            borderColor: p.tooltipBorder,
            textStyle: { color: p.tooltipText },
            formatter: (params: unknown) => {
                const arr = params as Array<{ dataIndex: number }>;
                const idx = arr[0]?.dataIndex ?? 0;
                const date = dates[idx];
                const row = date ? byDate.get(date) : undefined;
                const full = row?.full_runtime_seconds ?? 0;
                const sessions = row?.session_count ?? 0;
                return `<div style="font-weight:600">${labels[idx]} · ${date}</div>
<div>${formatDuration(full)} · ${sessions} session${sessions === 1 ? '' : 's'}</div>`;
            },
        },
        xAxis: {
            type: 'category',
            data: labels,
            axisLine: { lineStyle: { color: p.axis } },
            axisTick: { show: false },
            axisLabel: { color: p.axisLabel },
        },
        yAxis: {
            type: 'value',
            name: 'hours',
            nameTextStyle: { color: p.axisLabel, padding: [0, 0, 0, 32] },
            axisLabel: { color: p.axisLabel },
            axisLine: { show: false },
            splitLine: { lineStyle: { color: p.splitLine } },
        },
        series: [
            {
                type: 'bar',
                barWidth: '55%',
                itemStyle: { color: p.series[0], borderRadius: [4, 4, 0, 0] },
                data: hours,
            },
        ],
    };
}
