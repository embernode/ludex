// Presentation-layer helpers. Pure functions on primitive values,
// unit-testable without the Tauri runtime.

/**
 * Render a runtime in seconds as a compact string. Examples:
 * `0s`, `47s`, `12m`, `1h 23m`, `2d 3h`.
 */
export function formatSeconds(seconds: number): string {
    const s = Math.max(0, Math.floor(seconds));
    if (s < 60) return `${s}s`;
    if (s < 3_600) {
        const m = Math.floor(s / 60);
        const rem = s % 60;
        return rem === 0 ? `${m}m` : `${m}m ${rem}s`;
    }
    if (s < 86_400) {
        const h = Math.floor(s / 3_600);
        const m = Math.floor((s % 3_600) / 60);
        return m === 0 ? `${h}h` : `${h}h ${m}m`;
    }
    const d = Math.floor(s / 86_400);
    const h = Math.floor((s % 86_400) / 3_600);
    return h === 0 ? `${d}d` : `${d}d ${h}h`;
}

/**
 * How timestamps are rendered across the Games, Recent, and app-
 * detail views. Kept to a small enum rather than exposing every
 * `Intl.DateTimeFormat` option so the Settings UI is a short
 * dropdown, not a template-string editor.
 *
 * - `short`: locale-driven short form (`Apr 24, 2026, 6:30 PM` in
 *   en-US; locale determines 12h/24h).
 * - `iso`: tabular, unambiguous `YYYY-MM-DD HH:MM` in local time.
 *   Best when scanning a column of timestamps.
 * - `dmy`: day-first tabular `DD.MM.YYYY HH:MM`, common across
 *   German-speaking and Nordic locales.
 * - `relative`: `2 hours ago`, `yesterday`, `3 days ago`. Best when
 *   reading an entry in context.
 */
export type TimestampFormat = 'short' | 'iso' | 'dmy' | 'relative';

const TIMESTAMP_FORMAT_ATTR = 'timestampFormat';
const DEFAULT_TIMESTAMP_FORMAT: TimestampFormat = 'short';

/** Current timestamp format resolved from `<html data-timestamp-format>`. */
export function currentTimestampFormat(): TimestampFormat {
    if (typeof document === 'undefined') return DEFAULT_TIMESTAMP_FORMAT;
    const v = document.documentElement.dataset[TIMESTAMP_FORMAT_ATTR];
    if (v === 'iso' || v === 'dmy' || v === 'relative' || v === 'short') {
        return v;
    }
    return DEFAULT_TIMESTAMP_FORMAT;
}

/**
 * Invoke `callback` with the current format immediately, then again
 * every time `<html data-timestamp-format>` changes. Mirrors the
 * theme observer so pages can opt in with one line and re-render
 * when the user saves a new preference from Settings.
 */
export function observeTimestampFormat(
    callback: (format: TimestampFormat) => void,
): () => void {
    if (typeof document === 'undefined') return () => {};
    callback(currentTimestampFormat());
    const obs = new MutationObserver(() => callback(currentTimestampFormat()));
    obs.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ['data-timestamp-format'],
    });
    return () => obs.disconnect();
}

/**
 * Render an RFC 3339 UTC timestamp in the user's local timezone,
 * or `'—'` for an empty input (which the daemon uses to mean
 * "never" or "still open").
 *
 * Pass the caller's current [`TimestampFormat`] so Svelte's
 * reactivity re-runs the format call when the preference changes.
 */
export function formatTimestamp(
    iso: string,
    format: TimestampFormat = DEFAULT_TIMESTAMP_FORMAT,
): string {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    switch (format) {
        case 'iso':
            return formatIso(d);
        case 'dmy':
            return formatDmy(d);
        case 'relative':
            return formatRelative(d);
        case 'short':
        default:
            return d.toLocaleString(undefined, {
                year: 'numeric',
                month: 'short',
                day: 'numeric',
                hour: '2-digit',
                minute: '2-digit',
            });
    }
}

function pad2(n: number): string {
    return String(n).padStart(2, '0');
}

function formatIso(d: Date): string {
    return (
        `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ` +
        `${pad2(d.getHours())}:${pad2(d.getMinutes())}`
    );
}

function formatDmy(d: Date): string {
    return (
        `${pad2(d.getDate())}.${pad2(d.getMonth() + 1)}.${d.getFullYear()} ` +
        `${pad2(d.getHours())}:${pad2(d.getMinutes())}`
    );
}

// Thresholds, in milliseconds. Kept as module constants so the unit
// cascade is easy to read in one place.
const MS_MINUTE = 60_000;
const MS_HOUR = 3_600_000;
const MS_DAY = 86_400_000;
const MS_MONTH = 2_592_000_000; // 30 days — close enough for "a month ago"
const MS_YEAR = 31_536_000_000;

function formatRelative(d: Date): string {
    const diff = d.getTime() - Date.now(); // past is negative
    const abs = Math.abs(diff);
    const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
    if (abs < MS_MINUTE) return rtf.format(Math.round(diff / 1_000), 'second');
    if (abs < MS_HOUR) return rtf.format(Math.round(diff / MS_MINUTE), 'minute');
    if (abs < MS_DAY) return rtf.format(Math.round(diff / MS_HOUR), 'hour');
    if (abs < MS_MONTH) return rtf.format(Math.round(diff / MS_DAY), 'day');
    if (abs < MS_YEAR) return rtf.format(Math.round(diff / MS_MONTH), 'month');
    return rtf.format(Math.round(diff / MS_YEAR), 'year');
}

/**
 * Interactive runtime as a percentage of full runtime, for the share
 * bars in the library and on the app-detail page.
 *
 * Clamped to `[0, 100]` and zero when there is no full runtime to
 * divide by, so a bar can never render wider than its track or
 * divide by zero on a session that recorded nothing.
 */
export function interactiveShare(interactive: number, full: number): number {
    if (full <= 0) return 0;
    return Math.max(0, Math.min(100, (interactive / full) * 100));
}
