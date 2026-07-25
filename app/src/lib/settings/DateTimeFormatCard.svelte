<script lang="ts">
    import {
        currentTimestampFormat,
        formatTimestamp,
        type TimestampFormat,
    } from '$lib/format';
    import SettingsCard from './SettingsCard.svelte';
    import SettingRow from './SettingRow.svelte';

    /** Reference timestamp so the user can see the format in action. */
    const TS_SAMPLE = new Date(Date.now() - 2 * 3_600_000).toISOString();

    let tsFormat = $state<TimestampFormat>(currentTimestampFormat());

    /** Stored in `localStorage` and mirrored on
     *  `<html data-timestamp-format>` so every page observing the
     *  attribute re-renders on change. Pure presentation — no daemon
     *  round-trip, which is why this card already committed on change
     *  before the rest of Settings did. */
    function save() {
        document.documentElement.dataset.timestampFormat = tsFormat;
        try {
            localStorage.setItem('ludex-timestamp-format', tsFormat);
        } catch (_) {
            // localStorage blocked; the change still applies to this
            // session, just won't persist across restarts.
        }
    }
</script>

<SettingsCard>
    <SettingRow
        label="Date format"
        help="Used across the dashboard axis, tooltips, and session lists."
    >
        {#snippet control()}
            <span class="preview">{formatTimestamp(TS_SAMPLE, tsFormat)}</span>
            <select
                bind:value={tsFormat}
                onchange={save}
                aria-label="Timestamp format"
            >
                <option value="short">Short</option>
                <option value="iso">ISO</option>
                <option value="dmy">Day-first</option>
                <option value="relative">Relative</option>
            </select>
        {/snippet}
    </SettingRow>
</SettingsCard>

<style>
    .preview {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 11.5px;
        color: var(--fg3);
    }

    select {
        appearance: none;
        -webkit-appearance: none;
        font: inherit;
        font-size: 12.5px;
        color: var(--fg2);
        background: var(--tile);
        border: 1px solid var(--line);
        border-radius: 6px;
        padding: 5px 9px;
        cursor: pointer;
    }

    select:focus-visible {
        outline: 2px solid var(--ac);
        outline-offset: -1px;
    }
</style>
