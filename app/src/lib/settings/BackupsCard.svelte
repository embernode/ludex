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
    import SettingsCard from './SettingsCard.svelte';
    import SettingRow from './SettingRow.svelte';
    import NumberSetting from './NumberSetting.svelte';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    /** Lower bound on the backup interval. Mirrors the daemon's
     *  scheduler floor — the setter clamps any smaller value to this
     *  before persisting. */
    const BACKUP_INTERVAL_FLOOR_HOURS = 1;

    let intervalHours = $state<number>(24);
    let retention = $state<number>(14);

    /** Directory + count + size summary. `null` while loading or when
     *  the daemon couldn't resolve a backup directory. */
    let stats = $state<BackupStats | null>(null);

    let backingUp = $state<boolean>(false);
    let backupMessage = $state<string>('');

    /** Mirrors the user's global timestamp preference so the "last
     *  snapshot" cell follows the same format as the rest of the app. */
    let tsFormat = $state<TimestampFormat>(currentTimestampFormat());

    async function load() {
        try {
            const [hours, ret, s] = await Promise.all([
                getBackupIntervalHours(),
                getBackupRetentionCount(),
                getBackupStats(),
            ]);
            intervalHours = Math.max(
                BACKUP_INTERVAL_FLOOR_HOURS,
                Math.round(hours),
            );
            retention = Math.max(1, Math.round(ret));
            stats = s;
        } catch (e) {
            onerror?.(String(e));
        }
    }

    async function refreshStats() {
        try {
            stats = await getBackupStats();
        } catch (e) {
            onerror?.(String(e));
        }
    }

    async function commitInterval(hours: number) {
        await setBackupIntervalHours(hours);
        intervalHours = hours;
    }

    async function commitRetention(count: number) {
        await setBackupRetentionCount(count);
        retention = count;
        // The daemon prunes immediately on save, so older snapshots
        // beyond the new count are already gone — refresh so count and
        // total size reflect that.
        await refreshStats();
    }

    async function backupNow() {
        backingUp = true;
        backupMessage = '';
        try {
            const path = await takeBackupNow();
            backupMessage = `Saved ${path.split('/').pop() ?? path}`;
            await refreshStats();
            setTimeout(() => (backupMessage = ''), 4000);
        } catch (e) {
            onerror?.(String(e));
        } finally {
            backingUp = false;
        }
    }

    async function openFolder() {
        if (!stats) return;
        try {
            await openBackupDirectory(stats.directory);
        } catch (e) {
            onerror?.(String(e));
        }
    }

    /** Format a byte count as a short human-readable string. Mirrors
     *  the CLI's `format_size` so both agree on the same file. */
    function formatBytes(bytes: number): string {
        const KIB = 1024;
        const MIB = 1024 * 1024;
        const GIB = 1024 * 1024 * 1024;
        if (bytes < KIB) return `${bytes} B`;
        const tenths = (n: number, unit: number) =>
            Math.floor((n * 10) / unit) / 10;
        if (bytes < MIB) return `${tenths(bytes, KIB).toFixed(1)} KiB`;
        if (bytes < GIB) return `${tenths(bytes, MIB).toFixed(1)} MiB`;
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

<SettingsCard
    title="Backups"
    subtitle="Snapshots of the database, kept locally."
>
    {#if stats}
        <div class="stats">
            <div class="cell">
                <div class="celllabel">SNAPSHOTS</div>
                <!-- One expression rather than a trailing {#if}: Svelte
                     trims the leading whitespace off a block's text, so
                     the separator rendered as "14· 68.8 MiB". -->
                <div class="cellvalue">
                    {stats.count > 0
                        ? `${stats.count} · ${formatBytes(stats.total_bytes)}`
                        : stats.count}
                </div>
            </div>
            <div class="cell">
                <div class="celllabel">LAST</div>
                <div class="cellvalue stamp">
                    {stats.latest_at
                        ? formatTimestamp(stats.latest_at, tsFormat)
                        : '—'}
                </div>
            </div>
            <div class="cell folder">
                <div class="foldertext">
                    <div class="celllabel">FOLDER</div>
                    <div class="path" title={stats.directory}>
                        {stats.directory}
                    </div>
                </div>
                <button type="button" class="btn" onclick={openFolder}>
                    Open
                </button>
            </div>
        </div>
    {/if}

    <NumberSetting
        label="Keep"
        help="Older snapshots are pruned on save."
        unit="snapshots"
        bounds={{ min: 1, max: 365 }}
        value={retention}
        commit={commitRetention}
    />

    <NumberSetting
        label="Interval"
        help="Daemon reschedules live on change."
        unit="h"
        bounds={{ min: BACKUP_INTERVAL_FLOOR_HOURS, max: 720 }}
        value={intervalHours}
        commit={commitInterval}
    />

    <SettingRow
        label="Manual snapshot"
        help={backupMessage || 'Writes one now, outside the schedule.'}
    >
        {#snippet control()}
            <button
                type="button"
                class="btn"
                onclick={backupNow}
                disabled={backingUp}
            >
                {backingUp ? 'Backing up…' : 'Back up now'}
            </button>
        {/snippet}
    </SettingRow>
</SettingsCard>

<style>
    .stats {
        display: flex;
        border-bottom: 1px solid var(--hair);
    }

    .cell {
        flex: 1;
        padding: 13px 16px;
        border-right: 1px solid var(--hair);
        min-width: 0;
    }

    .cell:last-child {
        border-right: 0;
    }

    .folder {
        flex: 1.4;
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
    }

    .foldertext {
        min-width: 0;
    }

    .celllabel {
        font-size: 10.5px;
        font-weight: 500;
        letter-spacing: 0.09em;
        color: var(--fg3);
        margin-bottom: 6px;
    }

    .cellvalue {
        font-size: 18px;
        font-weight: 600;
        line-height: 1;
        font-variant-numeric: tabular-nums;
    }

    /* A full timestamp is a long run of digits, and at the stat-number
       size it reads as one undifferentiated block. Monospace at text
       size separates the groups and stops it competing with the two
       actual numbers in the strip. Sized down rather than split into
       date + time because the user's format is theirs to choose, and
       one of the four options ("2 hours ago") has no such split. */
    .stamp {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 13px;
        font-weight: 500;
        line-height: 1.3;
        color: var(--fg2);
    }

    .path {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 11px;
        color: var(--fg2);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .btn {
        font-size: 12px;
        font-weight: 500;
        border-radius: 6px;
        padding: 5px 11px;
        cursor: pointer;
        color: var(--fg);
        background: var(--tile);
        border: 1px solid var(--line);
        flex: none;
    }
</style>
