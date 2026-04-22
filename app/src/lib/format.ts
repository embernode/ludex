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
 * Render an RFC 3339 UTC timestamp as a human-readable string in
 * the user's local timezone, or `'—'` for an empty input (which
 * the daemon uses to mean "never" or "still open").
 */
export function formatTimestamp(iso: string): string {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString(undefined, {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
    });
}
