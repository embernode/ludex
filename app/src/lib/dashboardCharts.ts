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
import type { TimestampFormat } from './format';

/** Full-date label for chart tooltips and axis ticks. Carries the
 *  year: the axis shows few enough ticks to afford it. */
function formatTooltipDate(iso: string, fmt: TimestampFormat): string {
    if (fmt === 'dmy') {
        const parts = iso.split('-');
        if (parts.length === 3) return `${parts[2]}.${parts[1]}.${parts[0]}`;
    }
    return iso; // YYYY-MM-DD covers iso, short, relative.
}

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
 * Shift a `YYYY-MM-DD` date string by N days. Pure calendar
 * arithmetic on the label — pinning the intermediate Date to UTC
 * midnight just keeps the maths free of DST and timezone effects.
 */
function shiftDate(iso: string, deltaDays: number): string {
    const d = new Date(`${iso}T00:00:00Z`);
    d.setUTCDate(d.getUTCDate() + deltaDays);
    return d.toISOString().slice(0, 10);
}

/**
 * Today's *local* calendar date as `YYYY-MM-DD`, matching the
 * daemon's local-day bucketing so the axis windows and the returned
 * rows agree on what "today" means.
 */
function today(): string {
    const d = new Date();
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Index DailyPlaytime rows by their date for O(1) lookups. */
function indexByDate(rows: readonly DailyPlaytime[]): Map<string, DailyPlaytime> {
    const m = new Map<string, DailyPlaytime>();
    for (const r of rows) m.set(r.date, r);
    return m;
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
    tsFormat: TimestampFormat,
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
                const dateLabel = formatTooltipDate(date, tsFormat);
                return `<div style="font-weight:600">${dateLabel}</div>
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
 * Daily full runtime over the last `days` days as bars.
 *
 * Bars rather than a line: the series zero-fills days with no play,
 * and a smoothed line drawn through those zeros implies a gentle
 * decline into a day that actually had nothing in it. A bar of height
 * zero reads as zero, which is the truth.
 */
export function buildRecentBarOption(
    rows: readonly DailyPlaytime[],
    theme: Theme,
    tsFormat: TimestampFormat,
    days = 30,
): EChartsCoreOption {
    const p = palette(theme);
    const byDate = indexByDate(rows);

    const dates: string[] = [];
    const hours: number[] = [];
    const start = shiftDate(today(), -(days - 1));
    for (let i = 0; i < days; i++) {
        const date = shiftDate(start, i);
        dates.push(date);
        hours.push(hoursOf(byDate.get(date)?.full_runtime_seconds ?? 0));
    }

    // Annotate the busiest day. Carried as a per-datum label on the
    // bar itself rather than a `markPoint`, so it is anchored to the
    // bar it describes and cannot drift out of the plot area.
    //
    // `>=` makes a tie resolve to the most recent day, which is the
    // more useful one to point at. A run of zeroes has no peak worth
    // naming, so `peakIdx` stays -1 and no datum gets a label.
    let peakIdx = -1;
    for (let i = 0; i < hours.length; i++) {
        if (hours[i] > 0 && (peakIdx === -1 || hours[i] >= hours[peakIdx])) {
            peakIdx = i;
        }
    }

    const data: Array<number | { value: number; label: object }> = [...hours];
    if (peakIdx !== -1) {
        data[peakIdx] = {
            value: hours[peakIdx],
            label: {
                show: true,
                position: 'top',
                distance: 5,
                // Hours with one decimal, matching the y-axis unit.
                formatter: `peak ${hours[peakIdx].toFixed(1)}h`,
                color: p.axisLabel,
                fontSize: 11,
            },
        };
    }

    return {
        backgroundColor: 'transparent',
        // The top gap carries the peak label, which sits above a bar
        // that reaches the top of the plot area.
        grid: { left: 44, right: 16, top: 30, bottom: 28 },
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
                const label = date ? formatTooltipDate(date, tsFormat) : '';
                return `<div style="font-weight:600">${label}</div>
<div>${full > 0 ? formatDuration(full) : 'no play'} · ${sessions} session${sessions === 1 ? '' : 's'}</div>`;
            },
        },
        xAxis: {
            type: 'category',
            data: dates,
            axisLine: { lineStyle: { color: p.axis } },
            axisTick: { show: false },
            axisLabel: {
                color: p.axisLabel,
                // 30 labels don't fit; show roughly one per week.
                interval: Math.floor(days / 4),
                formatter: (value: string) => formatTooltipDate(value, tsFormat),
            },
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
                type: 'bar',
                itemStyle: { color: p.series[0], borderRadius: [2, 2, 0, 0] },
                data,
            },
        ],
    };
}
