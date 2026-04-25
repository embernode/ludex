<script lang="ts">
    import {
        currentTimestampFormat,
        formatTimestamp,
        type TimestampFormat,
    } from '$lib/format';

    /** Reference timestamp so the user can see each format in action. */
    const TS_SAMPLE = new Date(Date.now() - 2 * 3_600_000).toISOString();

    let tsFormat = $state<TimestampFormat>(currentTimestampFormat());

    /** Stored in `localStorage` and mirrored on
     *  `<html data-timestamp-format>` so every page observing the
     *  attribute re-renders on change. Pure presentation —
     *  no daemon round-trip. */
    function save() {
        document.documentElement.dataset.timestampFormat = tsFormat;
        try {
            localStorage.setItem('ludex-timestamp-format', tsFormat);
        } catch (_) {
            // localStorage blocked; the change still applies to
            // this session, just won't persist across restarts.
        }
    }
</script>

<section class="settings-card">
    <h2>Date & time format</h2>
    <p class="description">
        How timestamps are rendered in the Games, Recent, and
        app-detail views. Short follows your system locale; ISO is
        tabular and unambiguous; Relative reads as "2 hours ago".
        Stored in-app only — no daemon round-trip.
    </p>
    <label class="field">
        <span class="field-label">Format</span>
        <select bind:value={tsFormat} onchange={save}>
            <option value="short">Short (locale)</option>
            <option value="iso">ISO (2026-04-24 18:30)</option>
            <option value="dmy">Day-first (24.04.2026 18:30)</option>
            <option value="relative">Relative (2 hours ago)</option>
        </select>
    </label>
    <p class="hint">
        Preview: {formatTimestamp(TS_SAMPLE, tsFormat)}
    </p>
</section>
