// Chart color palettes. Hand-picked to match the page's CSS tokens
// — ECharts doesn't read CSS custom properties directly, so we
// duplicate them here. Keeping a single `palette(theme)` accessor
// means every chart shares the same axis / gridline / series
// colors per theme.
//
// Both palettes render their charts with a transparent background
// so the page surface shows through; on the dark palette that's
// true black, which is exactly what the OLED user asked for.

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

const LIGHT: ChartPalette = {
    series: ['#1e40af', '#059669'],
    axis: '#e5e7eb',
    axisLabel: '#6b7280',
    splitLine: '#f3f4f6',
    tooltipBg: '#ffffff',
    tooltipText: '#111111',
    tooltipBorder: '#e5e7eb',
    heatmapRange: ['#e5e7eb', '#1e40af'],
    heatmapCellBorder: '#ffffff',
    heatmapEmpty: '#f3f4f6',
};

const DARK: ChartPalette = {
    series: ['#60a5fa', '#34d399'],
    axis: '#27272a',
    axisLabel: '#a1a1aa',
    splitLine: '#1a1a1d',
    tooltipBg: '#0f0f10',
    tooltipText: '#fafafa',
    tooltipBorder: '#27272a',
    heatmapRange: ['#1a1a1d', '#60a5fa'],
    heatmapCellBorder: '#000000',
    heatmapEmpty: '#18181b',
};

export function palette(theme: Theme): ChartPalette {
    return theme === 'dark' ? DARK : LIGHT;
}
