// Chart color palettes. Hand-picked to match the page's CSS tokens
// — ECharts doesn't read CSS custom properties directly, so we
// duplicate them here. Keeping a single `palette(theme)` accessor
// means every chart shares the same axis / gridline / series
// colors per theme.
//
// The values below are the design's graphite token set resolved
// against the default accent: neutrals are transcribed straight from
// the `--line` / `--fg2` / `--surface` scale, and the two
// accent-derived entries are the `color-mix()` results
// (`#29424d` / `#b0c3cb` are the heat ramp's lowest non-empty level,
// a 34% accent mix over `--lane`; `#457488` is the light scheme's
// 62% darkening of the accent toward `#20262b`).
//
// The neutrals stay literals — they don't vary with the accent. The
// accent-derived entries are resolved from the live `--ac` at build
// time so the charts follow the accent picker, and the literals below
// are the fallback for that resolution. See `resolveCssColor` for why
// resolving needs a canvas rather than just `getComputedStyle`.
//
// Both palettes render their charts with a transparent background
// so the page surface shows through.

import { resolveCssColor, type Theme } from './theme';

export interface ChartPalette {
    /** `series` colors, cycled in order. */
    readonly series: readonly string[];
    /** Axis line / border. */
    readonly axis: string;
    /** Axis tick labels. */
    readonly axisLabel: string;
    /** Gridline between values. */
    readonly splitLine: string;
    /** Tooltip background. */
    readonly tooltipBg: string;
    /** Tooltip text. */
    readonly tooltipText: string;
    /** Tooltip border. */
    readonly tooltipBorder: string;
    /** Heatmap gradient from low to high. */
    readonly heatmapRange: readonly [string, string];
    /** Heatmap cell border (separates days visually on the calendar). */
    readonly heatmapCellBorder: string;
    /** Background of empty/zero heatmap cells. */
    readonly heatmapEmpty: string;
}

// `series[0]` carries full runtime and is also the only color the
// week-bar chart uses, so it takes the accent itself rather than the
// dimmed idle tint — a solitary filled bar in the idle tint sits at
// roughly 1.5:1 against the card and disappears. `series[1]` is the
// dashed interactive overlay, kept neutral so it reads as a
// secondary annotation over the accent-colored band.
const LIGHT: ChartPalette = {
    series: ['#457488', '#5a6469'],
    axis: 'rgba(20, 26, 32, 0.13)',
    axisLabel: '#5a6469',
    splitLine: 'rgba(20, 26, 32, 0.08)',
    tooltipBg: '#ffffff',
    tooltipText: '#1b2126',
    tooltipBorder: 'rgba(20, 26, 32, 0.13)',
    heatmapRange: ['#b0c3cb', '#457488'],
    heatmapCellBorder: '#eef1f3',
    heatmapEmpty: '#e9edef',
};

const DARK: ChartPalette = {
    series: ['#5aa9c9', '#8b9298'],
    axis: 'rgba(255, 255, 255, 0.09)',
    axisLabel: '#8b9298',
    splitLine: 'rgba(255, 255, 255, 0.055)',
    tooltipBg: '#14181b',
    tooltipText: '#e7ebee',
    tooltipBorder: 'rgba(255, 255, 255, 0.09)',
    heatmapRange: ['#29424d', '#5aa9c9'],
    heatmapCellBorder: '#0b0d0f',
    heatmapEmpty: '#111517',
};

/**
 * Chart colours for the current scheme, with the accent-derived
 * entries read from the page's live tokens.
 *
 * Callers that need to re-render when the user picks a new accent
 * must depend on `currentAccent()` themselves — this reads the DOM,
 * which is not something Svelte can track.
 */
export function palette(theme: Theme): ChartPalette {
    const base = theme === 'dark' ? DARK : LIGHT;
    return {
        ...base,
        series: [
            resolveCssColor('var(--ac)', base.series[0]),
            base.series[1],
        ],
        heatmapRange: [
            resolveCssColor(
                'color-mix(in oklab, var(--ac) 34%, var(--lane))',
                base.heatmapRange[0],
            ),
            resolveCssColor('var(--ac)', base.heatmapRange[1]),
        ],
    };
}
