// Chart color palettes. Hand-picked to match the page's CSS tokens
// — ECharts doesn't read CSS custom properties directly, so we
// duplicate them here. Keeping a single `palette(theme)` accessor
// means every chart shares the same axis / gridline / series
// colors per theme.
//
// The values below are the design's graphite token set resolved
// against the default green accent: neutrals are transcribed
// straight from the `--line` / `--fg2` / `--surface` scale, and the
// two accent-derived entries are the `color-mix()` results
// (`#274732` / `#afc6b8` are the heat ramp's lowest non-empty level,
// a 34% accent mix over `--lane`; `#3f7d53` is the light scheme's
// 62% darkening of the accent toward `#20262b`).
//
// These are literals on purpose, and only correct while the accent
// is the default. Making them follow the user's accent is a later
// step of the redesign, and there is a trap waiting there: reading
// a `color-mix()` custom property back through `getComputedStyle`
// yields `oklab(...)`, which zrender cannot parse and ECharts
// silently renders as opaque black. Whatever resolves the accent at
// runtime must convert to sRGB first — painting the value into a
// 1x1 canvas and reading the pixel back works.
//
// Both palettes render their charts with a transparent background
// so the page surface shows through.

import type { Theme } from './theme';

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
    series: ['#3f7d53', '#5a6469'],
    axis: 'rgba(20, 26, 32, 0.13)',
    axisLabel: '#5a6469',
    splitLine: 'rgba(20, 26, 32, 0.08)',
    tooltipBg: '#ffffff',
    tooltipText: '#1b2126',
    tooltipBorder: 'rgba(20, 26, 32, 0.13)',
    heatmapRange: ['#afc6b8', '#3f7d53'],
    heatmapCellBorder: '#eef1f3',
    heatmapEmpty: '#e9edef',
};

const DARK: ChartPalette = {
    series: ['#4fb96a', '#8b9298'],
    axis: 'rgba(255, 255, 255, 0.09)',
    axisLabel: '#8b9298',
    splitLine: 'rgba(255, 255, 255, 0.055)',
    tooltipBg: '#14181b',
    tooltipText: '#e7ebee',
    tooltipBorder: 'rgba(255, 255, 255, 0.09)',
    heatmapRange: ['#274732', '#4fb96a'],
    heatmapCellBorder: '#0b0d0f',
    heatmapEmpty: '#111517',
};

export function palette(theme: Theme): ChartPalette {
    return theme === 'dark' ? DARK : LIGHT;
}
