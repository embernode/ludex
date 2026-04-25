<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        getBackupIntervalHours,
        getBackupRetentionCount,
        getBackupStats,
        onDaemonReconnected,
        openBackupDirectory,
        setBackupIntervalHours,
        setBackupRetentionCount,
        takeBackupNow,
        type BackupStats,
    } from '$lib/api';
    import {
        currentTimestampFormat,
        formatTimestamp,
        observeTimestampFormat,
        type TimestampFormat,
    } from '$lib/format';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    /** Lower bound on the backup interval. Mirrors the daemon's
     *  scheduler floor — the setter clamps any smaller value to this
     *  before persisting, so 0 in the input becomes 1 on save. */
    const BACKUP_INTERVAL_FLOOR_HOURS = 1;

    let savedIntervalHours = $state<number>(24);
    let intervalHours = $state<number>(24);
    let intervalStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    let savedRetention = $state<number>(14);
    let retention = $state<number>(14);
    let retentionStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    /** Directory + count + size summary. `null` while loading or when
     *  the daemon couldn't resolve a backup directory. */
    let stats = $state<BackupStats | null>(null);

    let backupNowStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
    let backupNowMessage = $state<string>('');

    /** Mirrors the user's global timestamp preference so the
     *  "Last snapshot" line follows the same format the rest of
     *  the app uses. Updated by the layout-attribute observer. */
    let tsFormat = $state<TimestampFormat>(currentTimestampFormat());

    const intervalDirty = $derived(
        Math.max(BACKUP_INTERVAL_FLOOR_HOURS, Math.round(savedIntervalHours)) !==
            intervalHours,
    );
    const retentionDirty = $derived(
        Math.max(1, Math.round(savedRetention)) !== retention,
    );

    async function load() {
        try {
            const [hours, ret, s] = await Promise.all([
                getBackupIntervalHours(),
                getBackupRetentionCount(),
                getBackupStats(),
            ]);
            savedIntervalHours = hours;
            intervalHours = Math.max(
                BACKUP_INTERVAL_FLOOR_HOURS,
                Math.round(hours),
            );
            savedRetention = ret;
            retention = Math.max(1, Math.round(ret));
            stats = s;
        } catch (e) {
            onerror?.(String(e));
        }
    }

    /** Pull a fresh `BackupStats` without touching the rest of the
     *  card. Called after manual snapshots and retention saves so
     *  the count / size / last-snapshot fields update without
     *  re-fetching the interval or retention values. */
    async function refreshStats() {
        try {
            stats = await getBackupStats();
        } catch (e) {
            onerror?.(String(e));
        }
    }

    async function saveInterval() {
        const hours = Math.max(
            BACKUP_INTERVAL_FLOOR_HOURS,
            Math.floor(intervalHours),
        );
        intervalStatus = 'saving';
        try {
            await setBackupIntervalHours(hours);
            savedIntervalHours = hours;
            intervalHours = hours;
            intervalStatus = 'saved';
            setTimeout(() => {
                if (intervalStatus === 'saved') intervalStatus = 'idle';
            }, 2500);
        } catch (e) {
            onerror?.(String(e));
            intervalStatus = 'error';
        }
    }

    async function saveRetention() {
        const count = Math.max(1, Math.floor(retention));
        retentionStatus = 'saving';
        try {
            await setBackupRetentionCount(count);
            savedRetention = count;
            retention = count;
            // The daemon prunes immediately on save so older snapshots
            // beyond the new count are gone — refresh the stats so
            // count and total size reflect that.
            await refreshStats();
            retentionStatus = 'saved';
            setTimeout(() => {
                if (retentionStatus === 'saved') retentionStatus = 'idle';
            }, 2500);
        } catch (e) {
            onerror?.(String(e));
            retentionStatus = 'error';
        }
    }

    async function backupNow() {
        backupNowStatus = 'saving';
        backupNowMessage = '';
        try {
            const path = await takeBackupNow();
            const filename = path.split('/').pop() ?? path;
            backupNowMessage = `Saved ${filename}`;
            backupNowStatus = 'saved';
            await refreshStats();
            setTimeout(() => {
                if (backupNowStatus === 'saved') backupNowStatus = 'idle';
            }, 4000);
        } catch (e) {
            onerror?.(String(e));
            backupNowStatus = 'error';
            backupNowMessage = '';
        }
    }

    async function openBackupFolder() {
        if (!stats) return;
        try {
            await openBackupDirectory(stats.directory);
        } catch (e) {
            onerror?.(String(e));
        }
    }

    /** Format a byte count as a short human-readable string.
     *  Mirrors the daemon CLI's `format_size` so the GUI and CLI
     *  agree on labels for the same file sizes. */
    function formatBytes(bytes: number): string {
        const KIB = 1024;
        const MIB_ = 1024 * 1024;
        const GIB = 1024 * 1024 * 1024;
        if (bytes < KIB) return `${bytes} B`;
        const tenths = (n: number, unit: number) =>
            Math.floor((n * 10) / unit) / 10;
        if (bytes < MIB_) return `${tenths(bytes, KIB).toFixed(1)} KiB`;
        if (bytes < GIB) return `${tenths(bytes, MIB_).toFixed(1)} MiB`;
        return `${tenths(bytes, GIB).toFixed(1)} GiB`;
    }

    onMount(() => {
        load();
        const unobserveTs = observeTimestampFormat((f) => (tsFormat = f));
        const unlistens: Promise<UnlistenFn>[] = [onDaemonReconnected(load)];
        return () => {
            unobserveTs();
            for (const p of unlistens) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<section class="settings-card">
    <h2>Backups</h2>
    <p class="description">
        The daemon snapshots your database on a fixed cadence and
        once more on a clean shutdown. Snapshots are written to a
        local directory; nothing leaves your machine.
    </p>

    {#if stats}
        <dl class="backup-facts">
            <dt>Snapshots</dt>
            <dd>
                {stats.count}
                {#if stats.count > 0}
                    · {formatBytes(stats.total_bytes)} on disk
                {/if}
            </dd>
            <dt>Last snapshot</dt>
            <dd>
                {stats.latest_at
                    ? formatTimestamp(stats.latest_at, tsFormat)
                    : '—'}
            </dd>
            <dt>Folder</dt>
            <dd class="backup-path">
                <code>{stats.directory}</code>
                <button
                    type="button"
                    class="link-button"
                    onclick={openBackupFolder}
                >
                    Open
                </button>
            </dd>
        </dl>
    {/if}

    <label class="field">
        <span class="field-label">Snapshots to keep</span>
        <input
            type="number"
            min="1"
            max="365"
            step="1"
            bind:value={retention}
        />
    </label>
    <div class="actions">
        <button
            type="button"
            onclick={saveRetention}
            disabled={!retentionDirty || retentionStatus === 'saving'}
        >
            {#if retentionStatus === 'saving'}Saving…{:else}Save retention{/if}
        </button>
        {#if retentionStatus === 'saved'}
            <span class="hint">Saved.</span>
        {:else if retentionDirty}
            <span class="hint">Unsaved change.</span>
        {/if}
    </div>

    <label class="field">
        <span class="field-label">Interval (hours)</span>
        <input
            type="number"
            min={BACKUP_INTERVAL_FLOOR_HOURS}
            max="720"
            step="1"
            bind:value={intervalHours}
        />
    </label>
    <div class="actions">
        <button
            type="button"
            onclick={saveInterval}
            disabled={!intervalDirty || intervalStatus === 'saving'}
        >
            {#if intervalStatus === 'saving'}Saving…{:else}Save interval{/if}
        </button>
        {#if intervalStatus === 'saved'}
            <span class="hint">Saved.</span>
        {:else if intervalDirty}
            <span class="hint">Unsaved change.</span>
        {/if}
    </div>

    <div class="actions">
        <button
            type="button"
            onclick={backupNow}
            disabled={backupNowStatus === 'saving'}
        >
            {#if backupNowStatus === 'saving'}Backing up…{:else}Back up now{/if}
        </button>
        {#if backupNowStatus === 'saved' && backupNowMessage}
            <span class="hint">{backupNowMessage}</span>
        {/if}
    </div>
</section>

<style>
    .backup-facts {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 0.3rem 1rem;
        margin: 0 0 1rem;
        font-size: 0.88rem;
    }

    .backup-facts dt {
        color: var(--text-subtle);
        text-transform: uppercase;
        font-size: 0.75rem;
        letter-spacing: 0.03em;
        align-self: center;
    }

    .backup-facts dd {
        margin: 0;
        color: var(--text-secondary);
    }

    .backup-path {
        display: flex;
        align-items: center;
        gap: 0.6rem;
        flex-wrap: wrap;
    }

    .backup-path code {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        background: var(--code-bg);
        color: var(--code-text);
        padding: 0.1rem 0.35rem;
        border-radius: 4px;
        font-size: 0.8rem;
        word-break: break-all;
    }
</style>
