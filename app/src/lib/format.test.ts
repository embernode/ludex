import { describe, expect, it } from 'vitest';
import {
    formatHoursMinutes,
    outcomeLabel,
    relativeDayName,
    sharePercent,
} from './format';

// Drives the width of every share bar: interactive over full in the
// library and on the game-detail page, session over day total in the
// activity log. A value outside [0, 100] renders a bar wider than its
// own track.
describe('sharePercent', () => {
    it('gives the part as a percentage of the whole', () => {
        expect(sharePercent(50, 100)).toBe(50);
        expect(sharePercent(91, 100)).toBe(91);
    });

    it('is zero when the part is nothing', () => {
        expect(sharePercent(0, 100)).toBe(0);
    });

    it('is full when the part is the whole', () => {
        expect(sharePercent(100, 100)).toBe(100);
    });

    // A session that recorded no runtime at all — an immediate crash,
    // or a row still being written — must not divide by zero. Same for
    // a day whose sessions all recorded nothing.
    it('is zero when there is no whole to divide by', () => {
        expect(sharePercent(0, 0)).toBe(0);
        expect(sharePercent(10, 0)).toBe(0);
        expect(sharePercent(10, -5)).toBe(0);
    });

    // Interactive should never exceed full, but the two are summed by
    // separate paths in the daemon, so the bar clamps rather than
    // trusting the arithmetic.
    it('clamps rather than overflowing the track', () => {
        expect(sharePercent(150, 100)).toBe(100);
        expect(sharePercent(-10, 100)).toBe(0);
    });
});

// Day totals: seconds are noise at this scale, and a column of
// `4h 30m 12s` beside `2h 5s` is unreadable.
describe('formatHoursMinutes', () => {
    it('gives hours and minutes, minutes zero-padded', () => {
        expect(formatHoursMinutes(4 * 3600 + 30 * 60)).toBe('4h 30m');
        expect(formatHoursMinutes(5 * 3600)).toBe('5h 00m');
        expect(formatHoursMinutes(2 * 3600 + 5 * 60)).toBe('2h 05m');
    });

    it('drops the hours entirely under an hour', () => {
        expect(formatHoursMinutes(45 * 60)).toBe('45m');
    });

    // Rounds rather than truncates, so 59m 40s reads as the hour it
    // very nearly is instead of losing 40 seconds silently.
    it('rounds the seconds to the nearest minute', () => {
        expect(formatHoursMinutes(59 * 60 + 40)).toBe('1h 00m');
        expect(formatHoursMinutes(90)).toBe('2m');
        expect(formatHoursMinutes(89)).toBe('1m');
    });

    // A day total can exceed 24h: two games played at once, or simply
    // several long sessions summed. No day unit — this reads as
    // playtime, not as a date.
    it('keeps counting in hours past a full day', () => {
        expect(formatHoursMinutes(26 * 3600 + 10 * 60)).toBe('26h 10m');
    });

    // Rounding to zero would print `0m` next to real playtime and read
    // as none at all.
    it('does not round a real total away to nothing', () => {
        expect(formatHoursMinutes(20)).toBe('<1m');
        expect(formatHoursMinutes(0)).toBe('0m');
        expect(formatHoursMinutes(-5)).toBe('0m');
    });
});

// Heads the log's day groups. Compared on local calendar days, which
// is how the daemon buckets playtime — comparing instants would call
// a session eight hours ago "today" or not depending on the hour.
describe('relativeDayName', () => {
    const at = (y: number, m: number, d: number, h = 12) =>
        new Date(y, m - 1, d, h).toISOString();
    const now = new Date(2026, 6, 26, 14, 30); // Sun 26 Jul 2026, local

    it('names the current and previous day relatively', () => {
        expect(relativeDayName(at(2026, 7, 26), now)).toBe('Today');
        expect(relativeDayName(at(2026, 7, 25), now)).toBe('Yesterday');
    });

    // Anything older is a weekday, which is what the eye scans a log by.
    it('names older days by weekday', () => {
        expect(relativeDayName(at(2026, 7, 22), now)).toBe('Wednesday');
    });

    // A session at 00:05 today is still today, and one at 23:55
    // yesterday is still yesterday — the boundary is the calendar day,
    // not a 24-hour window.
    it('splits on local midnight rather than elapsed hours', () => {
        expect(relativeDayName(at(2026, 7, 26, 0), now)).toBe('Today');
        expect(relativeDayName(at(2026, 7, 25, 23), now)).toBe('Yesterday');
    });

    it('gives nothing for an unparseable timestamp', () => {
        expect(relativeDayName('not a date', now)).toBe('');
    });
});

// The daemon stores `ExitReason` as snake_case; the log prints prose.
describe('outcomeLabel', () => {
    it('reads each stored reason as a sentence', () => {
        expect(outcomeLabel('terminated')).toBe('Ended normally');
        expect(outcomeLabel('recovered')).toBe('Recovered after crash');
        expect(outcomeLabel('foreground_changed')).toBe('Switched away');
        expect(outcomeLabel('sleep_split')).toBe('Split by suspend');
    });

    // A session with no reason is still running.
    it('calls a session with no reason open', () => {
        expect(outcomeLabel(null)).toBe('Open');
        expect(outcomeLabel(undefined)).toBe('Open');
    });

    // A variant added daemon-side must not render as a blank cell
    // while the GUI catches up.
    it('falls back to the raw reason it does not know', () => {
        expect(outcomeLabel('some_new_reason')).toBe('some new reason');
    });
});
