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

/**
 * The date half of a timestamp, in the user's chosen format.
 *
 * Fields that carry a calendar date and no time of day — first
 * detection, a log day heading — need this rather than
 * `formatTimestamp`, whose trailing `18:12` is noise on a date. Both
 * read the same single preference, so a date-only field still follows
 * Settings instead of hardcoding a locale default.
 *
 * `relative` renders relatively, which is the point of choosing it.
 * Callers that pair this with their own relative label should pass an
 * absolute format instead, or the two will say the same thing twice.
 */
export function formatDate(
    iso: string,
    format: TimestampFormat = DEFAULT_TIMESTAMP_FORMAT,
): string {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    switch (format) {
        case 'iso':
            return isoDate(d);
        case 'dmy':
            return dmyDate(d);
        case 'relative':
            return formatRelative(d);
        case 'short':
        default:
            return d.toLocaleDateString(undefined, {
                year: 'numeric',
                month: 'short',
                day: 'numeric',
            });
    }
}

/**
 * The time-of-day half of a timestamp, as a clock.
 *
 * Deliberately ignores `relative`: a column of clock times is what
 * makes a log readable, and "2 hours ago – 1 hour ago" tells the
 * reader nothing about when a session actually ran. The tabular
 * formats give 24-hour; `short` and `relative` defer to the locale,
 * which is where a 12-hour clock comes from.
 *
 * Returns an em dash for empty input, which the daemon uses to mean a
 * session is still open.
 */
export function formatTime(
    iso: string,
    format: TimestampFormat = DEFAULT_TIMESTAMP_FORMAT,
): string {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '—';
    if (format === 'iso' || format === 'dmy') {
        return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
    }
    return d.toLocaleTimeString(undefined, {
        hour: '2-digit',
        minute: '2-digit',
    });
}

function pad2(n: number): string {
    return String(n).padStart(2, '0');
}

function isoDate(d: Date): string {
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function dmyDate(d: Date): string {
    return `${pad2(d.getDate())}.${pad2(d.getMonth() + 1)}.${d.getFullYear()}`;
}

function formatIso(d: Date): string {
    return `${isoDate(d)} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function formatDmy(d: Date): string {
    return `${dmyDate(d)} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
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
 * One duration as a percentage of another, for the share bars.
 *
 * The whole differs by view: the library and the game-detail list
 * divide interactive runtime by full runtime, while the activity log
 * divides a session by its day's total, so each bar reads as that
 * session's share of the day.
 *
 * Clamped to `[0, 100]` and zero when there is no whole to divide by,
 * so a bar can never render wider than its track or divide by zero on
 * a session that recorded nothing.
 */
export function sharePercent(part: number, whole: number): number {
    if (whole <= 0) return 0;
    return Math.max(0, Math.min(100, (part / whole) * 100));
}

/**
 * Heading for a log day group: `Today`, `Yesterday`, or the weekday
 * name for anything older.
 *
 * Compared on *local calendar days* rather than elapsed milliseconds,
 * matching how the daemon buckets playtime. A 24-hour window would
 * call a session "today" or not depending on the hour it started,
 * which is not what a reader means by the word.
 *
 * Returns the empty string for a timestamp that will not parse, so a
 * bad row heads its group with nothing rather than `Invalid Date`.
 */
export function relativeDayName(startedAt: string, now = new Date()): string {
    const d = new Date(startedAt);
    if (Number.isNaN(d.getTime())) return '';
    const midnight = (v: Date) =>
        new Date(v.getFullYear(), v.getMonth(), v.getDate()).getTime();
    const days = Math.round((midnight(now) - midnight(d)) / 86_400_000);
    if (days === 0) return 'Today';
    if (days === 1) return 'Yesterday';
    return d.toLocaleDateString(undefined, { weekday: 'long' });
}

/** Prose for a stored `ExitReason`, keyed by its snake_case wire form. */
const OUTCOMES: Record<string, string> = {
    terminated: 'Ended normally',
    foreground_changed: 'Switched away',
    recovered: 'Recovered after crash',
    sleep_split: 'Split by suspend',
};

/**
 * How a session ended, as a sentence rather than an enum variant.
 *
 * An unknown reason falls back to its own text with the underscores
 * opened out: the daemon can grow a variant before the GUI knows
 * about it, and a blank cell would read as missing data rather than
 * as a value this build cannot name.
 */
export function outcomeLabel(exitReason: string | null | undefined): string {
    if (!exitReason) return 'Open';
    return OUTCOMES[exitReason] ?? exitReason.replace(/_/g, ' ');
}

/**
 * A duration for display, seconds rounded away: `3d 5h` / `2h 05m` /
 * `45m`.
 *
 * This is what every user-facing duration uses apart from the live
 * session pill, which keeps seconds so a counter under a minute old
 * doesn't look frozen. Seconds carry no useful precision in a figure
 * being compared against others, and a column mixing `3h` with
 * `2m 52s` cannot be scanned.
 *
 * Unlike [`formatHoursMinutes`] this keeps a **day** unit, because
 * these values legitimately span days — a game's total runtime, its
 * longest session — and `1247h 30m` is not readable. Minutes are
 * dropped at day scale for the same reason.
 */
export function formatDuration(seconds: number): string {
    const s = Math.max(0, seconds);
    const totalMinutes = Math.round(s / 60);
    if (totalMinutes === 0) return s > 0 ? '<1m' : '0m';
    const d = Math.floor(totalMinutes / 1440);
    const rem = totalMinutes % 1440;
    const h = Math.floor(rem / 60);
    const m = rem % 60;
    if (d > 0) return h === 0 ? `${d}d` : `${d}d ${h}h`;
    if (h > 0) return `${h}h ${pad2(m)}m`;
    return `${m}m`;
}

/**
 * A day's playtime as hours and minutes, e.g. `4h 30m` / `5h 00m` /
 * `45m`.
 *
 * Seconds are dropped because a day total does not carry meaningful
 * precision at that scale, and a column mixing `4h 30m 12s` with
 * `2h 5s` cannot be scanned. They are *rounded* rather than truncated,
 * so `59m 40s` reads as the hour it nearly is.
 *
 * Minutes are zero-padded beside an hours figure so the column stays
 * aligned. There is no day unit: a day's total can pass 24 hours when
 * sessions overlap or simply stack up, and `1d 2h` would read as a
 * date rather than as an amount of play.
 */
export function formatHoursMinutes(seconds: number): string {
    const s = Math.max(0, seconds);
    const totalMinutes = Math.round(s / 60);
    // Real play that rounds to nothing would otherwise print `0m` and
    // read as a day with none at all.
    if (totalMinutes === 0) return s > 0 ? '<1m' : '0m';
    const h = Math.floor(totalMinutes / 60);
    const m = totalMinutes % 60;
    return h === 0 ? `${m}m` : `${h}h ${String(m).padStart(2, '0')}m`;
}
