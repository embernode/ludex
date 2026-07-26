import { describe, expect, it } from 'vitest';
import {
    formatDate,
    formatDuration,
    formatHoursMinutes,
    formatTime,
    formatTimeRange,
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

// The date-only and time-only renderers exist so fields that need one
// half of a timestamp still follow the user's format preference, rather
// than hardcoding a locale default and ignoring the setting.
describe('formatDate', () => {
    // Built from local parts: the renderers work in local time, so a
    // literal UTC string would land on the wrong day west of UTC.
    const at = new Date(2026, 6, 24, 18, 12).toISOString();

    it('renders the tabular formats without a time', () => {
        expect(formatDate(at, 'iso')).toBe('2026-07-24');
        expect(formatDate(at, 'dmy')).toBe('24.07.2026');
    });

    it('carries no clock in the locale-driven form', () => {
        const out = formatDate(at, 'short');
        expect(out).toContain('2026');
        expect(out).not.toMatch(/\d{1,2}:\d{2}/);
    });

    // A date-only field under the relative preference should read
    // relatively — that is the whole point of choosing it.
    it('reads relatively when that is the preference', () => {
        const daysAgo = new Date();
        daysAgo.setDate(daysAgo.getDate() - 3);
        expect(formatDate(daysAgo.toISOString(), 'relative')).toMatch(/ago|days?/i);
    });

    it('handles the daemon empty string and unparseable input', () => {
        expect(formatDate('', 'iso')).toBe('—');
        expect(formatDate('not a date', 'iso')).toBe('not a date');
    });
});

describe('formatTime', () => {
    const at = new Date(2026, 6, 24, 18, 12).toISOString();

    it('gives a 24-hour clock for the tabular formats', () => {
        expect(formatTime(at, 'iso')).toBe('18:12');
        expect(formatTime(at, 'dmy')).toBe('18:12');
    });

    it('gives a clock for the locale-driven form', () => {
        expect(formatTime(at, 'short')).toMatch(/\d{1,2}:\d{2}/);
    });

    // `relative` has no notion of a time of day, and a log row reading
    // "2 hours ago – 1 hour ago" is useless. A clock column stays a
    // clock whatever the preference says.
    it('stays a clock under the relative preference', () => {
        const out = formatTime(at, 'relative');
        expect(out).toMatch(/\d{1,2}:\d{2}/);
        expect(out).not.toMatch(/ago/i);
    });

    it('handles the daemon empty string and unparseable input', () => {
        expect(formatTime('', 'iso')).toBe('—');
        expect(formatTime('not a date', 'iso')).toBe('—');
    });
});

// The session list shows one date and a clock range rather than two
// full timestamps. That drops the end date, so a session running past
// midnight has to say so or `23:30 – 01:15` reads as a 22-hour session
// backwards.
describe('formatTimeRange', () => {
    const at = (d: number, h: number, m = 0) =>
        new Date(2026, 6, d, h, m).toISOString();

    it('gives a plain range within one day', () => {
        expect(formatTimeRange(at(24, 20), at(24, 23), 'iso')).toBe(
            '20:00 – 23:00',
        );
    });

    it('marks how many days later the session ended', () => {
        expect(formatTimeRange(at(24, 23, 30), at(25, 1, 15), 'iso')).toBe(
            '23:30 – 01:15 +1 day',
        );
        expect(formatTimeRange(at(24, 22), at(26, 6), 'iso')).toBe(
            '22:00 – 06:00 +2 days',
        );
    });

    // Counts calendar days, not elapsed 24-hour blocks: 23:30 to 01:15
    // is under two hours but lands on the next date.
    it('counts the date boundary rather than elapsed hours', () => {
        expect(formatTimeRange(at(24, 0, 5), at(24, 23, 55), 'iso')).toBe(
            '00:05 – 23:55',
        );
    });

    // An open session has no end at all; the daemon sends an empty
    // string and there is no day offset to report.
    it('leaves an open session without an end', () => {
        expect(formatTimeRange(at(24, 20), '', 'iso')).toBe('20:00 – —');
    });

    it('gives an em dash when the start will not parse', () => {
        expect(formatTimeRange('', at(24, 23), 'iso')).toBe('—');
        expect(formatTimeRange('not a date', '', 'iso')).toBe('—');
    });
});

// Every duration shown to the user except the live pill. Drops
// seconds like formatHoursMinutes, but keeps a day unit because these
// values legitimately span days — a game's total runtime, its longest
// session — where an hours-only figure becomes unreadable.
describe('formatDuration', () => {
    const H = 3600;
    const D = 86_400;

    it('keeps days for values that span them', () => {
        expect(formatDuration(3 * D + 5 * H)).toBe('3d 5h');
        expect(formatDuration(3 * D)).toBe('3d');
    });

    it('gives padded hours and minutes below a day', () => {
        expect(formatDuration(2 * H + 5 * 60)).toBe('2h 05m');
        expect(formatDuration(2 * H)).toBe('2h 00m');
    });

    it('gives bare minutes below an hour', () => {
        expect(formatDuration(45 * 60)).toBe('45m');
    });

    it('rounds the seconds away rather than truncating', () => {
        expect(formatDuration(59 * 60 + 40)).toBe('1h 00m');
        expect(formatDuration(40)).toBe('1m');
        // Rounds up across the day boundary too, rather than showing
        // 24h in a formatter that has a day unit.
        expect(formatDuration(D - 20)).toBe('1d');
    });

    it('does not round a real duration away to nothing', () => {
        expect(formatDuration(20)).toBe('<1m');
        expect(formatDuration(0)).toBe('0m');
        expect(formatDuration(-5)).toBe('0m');
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
