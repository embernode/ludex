import { describe, expect, it } from 'vitest';
import { buildRecentBarOption } from './dashboardCharts';
import type { DailyPlaytime } from './api';

/** Local calendar date N days before today, as `YYYY-MM-DD`. The
 *  builder windows on the local day, so the fixtures have to be
 *  built the same way rather than from a fixed literal. */
function daysAgo(n: number): string {
    const d = new Date();
    d.setDate(d.getDate() - n);
    const pad = (v: number) => String(v).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function row(date: string, hours: number): DailyPlaytime {
    return {
        date,
        full_runtime_seconds: Math.round(hours * 3600),
        interactive_runtime_seconds: Math.round(hours * 3600),
        session_count: 1,
    };
}

/** The bar series' data array, which mixes plain numbers with a
 *  labelled object on the peak day. */
function barData(rows: DailyPlaytime[]): Array<unknown> {
    const option = buildRecentBarOption(rows, 'dark', 'iso') as {
        series: Array<{ data: unknown[] }>;
    };
    return option.series[0].data;
}

/** The single datum carrying a label, or undefined if none does. */
function labelled(data: Array<unknown>) {
    return data.find(
        (d): d is { value: number; label: { formatter: string } } =>
            typeof d === 'object' && d !== null && 'label' in d,
    );
}

describe('buildRecentBarOption peak annotation', () => {
    it('labels the highest day with its runtime in hours', () => {
        const data = barData([
            row(daysAgo(5), 1),
            row(daysAgo(3), 3.4),
            row(daysAgo(1), 2),
        ]);
        const peak = labelled(data);
        expect(peak).toBeDefined();
        expect(peak?.value).toBeCloseTo(3.4, 5);
        expect(peak?.label.formatter).toBe('peak 3.4h');
    });

    // A day of nothing is not a peak. Annotating a flat run of zeroes
    // would print `peak 0.0h` over an empty chart.
    it('annotates nothing when no day has any play', () => {
        expect(labelled(barData([row(daysAgo(2), 0)]))).toBeUndefined();
        expect(labelled(barData([]))).toBeUndefined();
    });

    // Every other day stays a bare number, so only one label is drawn.
    it('leaves the non-peak days unlabelled', () => {
        const data = barData([row(daysAgo(4), 2), row(daysAgo(2), 5)]);
        expect(data.filter((d) => typeof d === 'object' && d !== null)).toHaveLength(1);
    });

    // Ties resolve to the most recent day: when two days match, the
    // later one is the more useful thing to point at.
    it('picks the most recent day when two share the maximum', () => {
        const data = barData([row(daysAgo(6), 4), row(daysAgo(2), 4)]);
        const peak = labelled(data);
        expect(data.indexOf(peak as unknown)).toBe(data.length - 3);
    });

    // The annotation sits above the tallest bar, which reaches the top
    // of the plot area — so the grid has to reserve room for it.
    it('leaves headroom above the plot for the label', () => {
        const option = buildRecentBarOption([row(daysAgo(1), 3)], 'dark', 'iso') as {
            grid: { top: number };
        };
        expect(option.grid.top).toBeGreaterThanOrEqual(28);
    });
});
