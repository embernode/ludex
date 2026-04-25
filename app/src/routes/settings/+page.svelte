<script lang="ts">
    import { onMount } from 'svelte';
    import { getVersion } from '@tauri-apps/api/app';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import {
        blockApplication,
        getAltTabGraceSeconds,
        getBackupIntervalHours,
        getBackupRetentionCount,
        getBackupStats,
        getGpuMemoryThresholdBytes,
        getIdleGraceSeconds,
        getPauseWhenBackgrounded,
        listApplications,
        listBlockedApplicationIds,
        onBlocklistChanged,
        onDaemonReconnected,
        openBackupDirectory,
        setAltTabGraceSeconds,
        setBackupIntervalHours,
        setBackupRetentionCount,
        setGpuMemoryThresholdBytes,
        setIdleGraceSeconds,
        setPauseWhenBackgrounded,
        takeBackupNow,
        unblockApplication,
        type ApplicationSummary,
        type BackupStats,
    } from '$lib/api';
    import {
        currentTimestampFormat,
        formatTimestamp,
        type TimestampFormat,
    } from '$lib/format';

    /** MiB <-> bytes (we show mebibytes in the UI). */
    const MIB = 1024 * 1024;

    let apps = $state<ApplicationSummary[]>([]);
    let blocked = $state<Set<number>>(new Set());
    let loading = $state(true);
    /** True once the initial `load()` populated the page state. Lets
     *  the template distinguish "load failed before we had anything
     *  to show" (full-page error) from "save failed mid-session"
     *  (inline banner that keeps the form visible so the user can
     *  retry without losing their typed values). */
    let loaded = $state(false);
    /** Reason the initial `load()` failed; renders as a full-page
     *  banner only while `loaded` is still false. */
    let loadError = $state<string | null>(null);
    /** Most recent per-action error (save, manual snapshot, etc.).
     *  Rendered as a dismissable inline banner above the sections —
     *  never replaces the form, so the user can retry in place. */
    let error = $state<string | null>(null);

    /** Bytes currently persisted, for dirty-check. */
    let savedThresholdBytes = $state<number>(0);
    /** MiB, edit-in-progress value bound to the input. */
    let thresholdMib = $state<number>(50);
    let thresholdStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    /** Seconds currently persisted, for dirty-check. */
    let savedGraceSeconds = $state<number>(0);
    /** Seconds, edit-in-progress value bound to the input. */
    let graceSeconds = $state<number>(15);
    let graceStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    /** Whether losing focus should pause the session. Saves
     *  immediately on toggle — no dirty-check / save button. */
    let pauseWhenBackgrounded = $state<boolean>(true);

    /** Per-idle-interval cutscene grace, in MINUTES (the underlying
     *  setting is stored as seconds; we expose minutes in the UI
     *  because a 5-second grace makes no sense and 5 minutes is
     *  the typical default). */
    let savedIdleGraceMinutes = $state<number>(5);
    let idleGraceMinutes = $state<number>(5);
    let idleGraceStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    /** Lower bound on the backup interval. Mirrors the daemon's
     *  scheduler floor — the setter clamps any smaller value to this
     *  before persisting, so 0 in the input becomes 1 on save. */
    const BACKUP_INTERVAL_FLOOR_HOURS = 1;

    /** Hours currently persisted, for dirty-check. */
    let savedBackupIntervalHours = $state<number>(24);
    let backupIntervalHours = $state<number>(24);
    let backupIntervalStatus = $state<'idle' | 'saving' | 'saved' | 'error'>(
        'idle',
    );

    /** Snapshots currently persisted, for dirty-check. */
    let savedBackupRetention = $state<number>(14);
    let backupRetention = $state<number>(14);
    let backupRetentionStatus = $state<'idle' | 'saving' | 'saved' | 'error'>(
        'idle',
    );

    /** Directory + count + size summary. `null` while loading or when
     *  the daemon couldn't resolve a backup directory. */
    let backupStats = $state<BackupStats | null>(null);

    let backupNowStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
    let backupNowMessage = $state<string>('');

    /**
     * Timestamp format preference. Stored in `localStorage` and
     * mirrored on `<html data-timestamp-format>` so every page
     * observing the attribute re-renders on change. Purely a
     * presentation concern — no daemon round-trip.
     */
    let tsFormat = $state<TimestampFormat>(currentTimestampFormat());

    /** A reference timestamp so the user can see each format in action. */
    const TS_SAMPLE = new Date(Date.now() - 2 * 3_600_000).toISOString();

    /** Version resolved from tauri.conf.json on mount. Empty until
     *  the Tauri API hands us the string. */
    let appVersion = $state<string>('');
    /** Public repo URL — kept in sync with `repository` in the
     *  workspace Cargo.toml. */
    const REPO_URL = 'https://github.com/embernode/ludex';

    async function openRepo() {
        try {
            await openUrl(REPO_URL);
        } catch (e) {
            error = String(e);
        }
    }

    /** Blocked-games list filter. Case-insensitive substring match
     *  against product name and publisher. */
    let filterQuery = $state<string>('');
    const visibleApps = $derived.by(() => {
        const q = filterQuery.trim().toLowerCase();
        if (!q) return apps;
        return apps.filter((a) => {
            if (a.product_name.toLowerCase().includes(q)) return true;
            if (a.publisher && a.publisher.toLowerCase().includes(q)) return true;
            return false;
        });
    });

    async function load() {
        loading = true;
        try {
            const [
                a,
                ids,
                threshold,
                grace,
                pause,
                idleGraceSecs,
                intervalHours,
                retention,
                stats,
            ] = await Promise.all([
                listApplications(),
                listBlockedApplicationIds(),
                getGpuMemoryThresholdBytes(),
                getAltTabGraceSeconds(),
                getPauseWhenBackgrounded(),
                getIdleGraceSeconds(),
                getBackupIntervalHours(),
                getBackupRetentionCount(),
                getBackupStats(),
            ]);
            apps = a;
            blocked = new Set(ids);
            savedThresholdBytes = threshold;
            thresholdMib = Math.max(1, Math.round(threshold / MIB));
            savedGraceSeconds = grace;
            graceSeconds = Math.max(0, Math.round(grace));
            pauseWhenBackgrounded = pause;
            // Round to whole minutes for the UI; the daemon stores
            // seconds, but a sub-minute grace is too short to matter.
            savedIdleGraceMinutes = Math.max(0, Math.round(idleGraceSecs / 60));
            idleGraceMinutes = savedIdleGraceMinutes;
            savedBackupIntervalHours = intervalHours;
            backupIntervalHours = Math.max(
                BACKUP_INTERVAL_FLOOR_HOURS,
                Math.round(intervalHours),
            );
            savedBackupRetention = retention;
            backupRetention = Math.max(1, Math.round(retention));
            backupStats = stats;
            loaded = true;
            loadError = null;
        } catch (e) {
            // Pre-`loaded` failure → full-page banner; mid-session
            // failures (e.g. the daemon dropped after a successful
            // first load and a reconnect retry hit it before the bus
            // came back) surface inline so the user keeps the form.
            if (loaded) {
                error = String(e);
            } else {
                loadError = String(e);
            }
        } finally {
            loading = false;
        }
    }

    /** Pull a fresh `BackupStats` without disturbing the rest of the
     *  page. Called after manual snapshots so the size/count/last
     *  fields update. Failures are surfaced through `error` so the
     *  user sees them — silent staleness would be more confusing. */
    async function refreshBackupStats() {
        try {
            backupStats = await getBackupStats();
        } catch (e) {
            error = String(e);
        }
    }

    async function togglePauseWhenBackgrounded() {
        // Toggle: the bind:checked on the input has already flipped
        // the state variable for us before this onchange fires.
        try {
            await setPauseWhenBackgrounded(pauseWhenBackgrounded);
            error = null;
        } catch (e) {
            // Revert on failure so the UI reflects reality.
            pauseWhenBackgrounded = !pauseWhenBackgrounded;
            error = String(e);
        }
    }

    async function toggleBlock(id: number) {
        try {
            if (blocked.has(id)) {
                await unblockApplication(id);
                blocked.delete(id);
            } else {
                await blockApplication(id);
                blocked.add(id);
            }
            // Reassign so Svelte sees the Set as a new value.
            blocked = new Set(blocked);
            error = null;
        } catch (e) {
            error = String(e);
        }
    }

    async function saveThreshold() {
        const bytes = Math.max(1, Math.floor(thresholdMib * MIB));
        thresholdStatus = 'saving';
        try {
            await setGpuMemoryThresholdBytes(bytes);
            savedThresholdBytes = bytes;
            thresholdStatus = 'saved';
            setTimeout(() => {
                if (thresholdStatus === 'saved') thresholdStatus = 'idle';
            }, 2500);
        } catch (e) {
            error = String(e);
            thresholdStatus = 'error';
        }
    }

    function saveTimestampFormat() {
        document.documentElement.dataset.timestampFormat = tsFormat;
        try {
            localStorage.setItem('ludex-timestamp-format', tsFormat);
        } catch (_) {
            // localStorage blocked; the change still applies to
            // this session, just won't persist across restarts.
        }
    }

    async function saveGrace() {
        const seconds = Math.max(0, Math.floor(graceSeconds));
        graceStatus = 'saving';
        try {
            await setAltTabGraceSeconds(seconds);
            savedGraceSeconds = seconds;
            graceStatus = 'saved';
            setTimeout(() => {
                if (graceStatus === 'saved') graceStatus = 'idle';
            }, 2500);
        } catch (e) {
            error = String(e);
            graceStatus = 'error';
        }
    }

    const thresholdDirty = $derived(
        Math.max(1, Math.round(savedThresholdBytes / MIB)) !== thresholdMib,
    );

    const graceDirty = $derived(
        Math.max(0, Math.round(savedGraceSeconds)) !== graceSeconds,
    );

    const idleGraceDirty = $derived(
        Math.max(0, Math.round(savedIdleGraceMinutes)) !== idleGraceMinutes,
    );

    async function saveIdleGrace() {
        const minutes = Math.max(0, Math.floor(idleGraceMinutes));
        const seconds = minutes * 60;
        idleGraceStatus = 'saving';
        try {
            await setIdleGraceSeconds(seconds);
            savedIdleGraceMinutes = minutes;
            idleGraceMinutes = minutes;
            idleGraceStatus = 'saved';
            setTimeout(() => {
                if (idleGraceStatus === 'saved') idleGraceStatus = 'idle';
            }, 2500);
        } catch (e) {
            error = String(e);
            idleGraceStatus = 'error';
        }
    }

    const backupIntervalDirty = $derived(
        Math.max(
            BACKUP_INTERVAL_FLOOR_HOURS,
            Math.round(savedBackupIntervalHours),
        ) !== backupIntervalHours,
    );

    const backupRetentionDirty = $derived(
        Math.max(1, Math.round(savedBackupRetention)) !== backupRetention,
    );

    async function saveBackupInterval() {
        const hours = Math.max(
            BACKUP_INTERVAL_FLOOR_HOURS,
            Math.floor(backupIntervalHours),
        );
        backupIntervalStatus = 'saving';
        try {
            await setBackupIntervalHours(hours);
            savedBackupIntervalHours = hours;
            backupIntervalHours = hours;
            backupIntervalStatus = 'saved';
            setTimeout(() => {
                if (backupIntervalStatus === 'saved') {
                    backupIntervalStatus = 'idle';
                }
            }, 2500);
        } catch (e) {
            error = String(e);
            backupIntervalStatus = 'error';
        }
    }

    async function saveBackupRetention() {
        const count = Math.max(1, Math.floor(backupRetention));
        backupRetentionStatus = 'saving';
        try {
            await setBackupRetentionCount(count);
            savedBackupRetention = count;
            backupRetention = count;
            // The daemon prunes immediately on save so older
            // snapshots beyond the new count are gone — refresh the
            // stats card so count and total size reflect that
            // without waiting for the next snapshot.
            await refreshBackupStats();
            backupRetentionStatus = 'saved';
            setTimeout(() => {
                if (backupRetentionStatus === 'saved') {
                    backupRetentionStatus = 'idle';
                }
            }, 2500);
        } catch (e) {
            error = String(e);
            backupRetentionStatus = 'error';
        }
    }

    async function backupNow() {
        backupNowStatus = 'saving';
        backupNowMessage = '';
        try {
            const path = await takeBackupNow();
            // The path is daemon-resolved; show only the filename
            // since the directory is rendered elsewhere on the card.
            const filename = path.split('/').pop() ?? path;
            backupNowMessage = `Saved ${filename}`;
            backupNowStatus = 'saved';
            await refreshBackupStats();
            setTimeout(() => {
                if (backupNowStatus === 'saved') {
                    backupNowStatus = 'idle';
                }
            }, 4000);
        } catch (e) {
            error = String(e);
            backupNowStatus = 'error';
            backupNowMessage = '';
        }
    }

    async function openBackupFolder() {
        if (!backupStats) return;
        try {
            await openBackupDirectory(backupStats.directory);
        } catch (e) {
            error = String(e);
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
        // Version read is fire-and-forget; an old Tauri without
        // this API just leaves the field blank.
        getVersion()
            .then((v) => {
                appVersion = v;
            })
            .catch(() => {});
        const unlisteners: Promise<UnlistenFn>[] = [
            onDaemonReconnected(load),
            onBlocklistChanged(load),
        ];
        return () => {
            for (const p of unlisteners) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<main>
    <header>
        <h1>Settings</h1>
    </header>

    {#if loading && !loaded}
        <p class="hint">Loading…</p>
    {:else if loadError && !loaded}
        <div class="error">
            <p><strong>Couldn't reach the daemon.</strong></p>
            <p class="detail">{loadError}</p>
            <p class="hint">Is <code>ludex-daemon</code> running?</p>
        </div>
    {:else}
        {#if error}
            <div class="error inline">
                <p class="detail">{error}</p>
                <button
                    type="button"
                    class="link-button"
                    onclick={() => (error = null)}
                    aria-label="Dismiss"
                >
                    Dismiss
                </button>
            </div>
        {/if}
        <section>
            <h2>Detection thresholds</h2>
            <p class="description">
                The foreground-window fallback accepts a non-fullscreen process as
                a game if it is using at least this much GPU memory. Raise it to
                keep quiet desktop apps out of your history; lower it to catch
                windowed games with small VRAM footprints.
            </p>
            <label class="field">
                <span class="field-label">GPU memory threshold (MiB)</span>
                <input
                    type="number"
                    min="1"
                    max="16384"
                    step="1"
                    bind:value={thresholdMib}
                />
            </label>
            <div class="actions">
                <button
                    type="button"
                    onclick={saveThreshold}
                    disabled={!thresholdDirty || thresholdStatus === 'saving'}
                >
                    {#if thresholdStatus === 'saving'}Saving…{:else}Save threshold{/if}
                </button>
                {#if thresholdStatus === 'saved'}
                    <span class="hint">Saved.</span>
                {:else if thresholdDirty}
                    <span class="hint">Unsaved change.</span>
                {/if}
            </div>
        </section>

        <section>
            <h2>Alt-tab grace window</h2>
            <p class="description">
                Seconds to wait after a tracked game loses focus before closing
                the session. Short alt-tabs to a browser or chat window stay
                inside one session; leaving the game for longer than the grace
                period ends it. Set to 0 to close sessions immediately on focus
                loss. Turn the toggle below off to never pause on focus loss —
                sessions will only end when the game process exits.
            </p>
            <label class="toggle">
                <input
                    type="checkbox"
                    bind:checked={pauseWhenBackgrounded}
                    onchange={togglePauseWhenBackgrounded}
                />
                <span>Pause session when the game loses focus</span>
            </label>
            <label class="field">
                <span class="field-label">Grace window (seconds)</span>
                <input
                    type="number"
                    min="0"
                    max="600"
                    step="1"
                    bind:value={graceSeconds}
                    disabled={!pauseWhenBackgrounded}
                />
            </label>
            <div class="actions">
                <button
                    type="button"
                    onclick={saveGrace}
                    disabled={!graceDirty ||
                        graceStatus === 'saving' ||
                        !pauseWhenBackgrounded}
                >
                    {#if graceStatus === 'saving'}Saving…{:else}Save grace window{/if}
                </button>
                {#if graceStatus === 'saved'}
                    <span class="hint">Saved.</span>
                {:else if graceDirty}
                    <span class="hint">Unsaved change.</span>
                {/if}
            </div>

            <p class="description sub-description">
                <strong>Cutscene grace.</strong> The first few minutes of
                any input-idle period are credited as interactive runtime
                rather than subtracted as AFK — covers cutscenes, dialogue
                trees, and long animations where you're watching but not
                pressing keys. Genuine AFK longer than this still bills
                correctly: only the first <code>N</code> minutes of each
                idle interval are forgiven, the rest is subtracted. Set
                to 0 to disable forgiveness and have every idle second
                subtracted as before.
            </p>
            <label class="field">
                <span class="field-label">Cutscene grace (minutes)</span>
                <input
                    type="number"
                    min="0"
                    max="60"
                    step="1"
                    bind:value={idleGraceMinutes}
                />
            </label>
            <div class="actions">
                <button
                    type="button"
                    onclick={saveIdleGrace}
                    disabled={!idleGraceDirty || idleGraceStatus === 'saving'}
                >
                    {#if idleGraceStatus === 'saving'}Saving…{:else}Save cutscene grace{/if}
                </button>
                {#if idleGraceStatus === 'saved'}
                    <span class="hint">Saved.</span>
                {:else if idleGraceDirty}
                    <span class="hint">Unsaved change.</span>
                {/if}
            </div>
        </section>

        <section>
            <h2>Date & time format</h2>
            <p class="description">
                How timestamps are rendered in the Games, Recent, and
                app-detail views. Short follows your system locale; ISO is
                tabular and unambiguous; Relative reads as "2 hours ago".
                Stored in-app only — no daemon round-trip.
            </p>
            <label class="field">
                <span class="field-label">Format</span>
                <select bind:value={tsFormat} onchange={saveTimestampFormat}>
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

        <section>
            <h2>Backups</h2>
            <p class="description">
                The daemon snapshots your database on a fixed cadence and
                once more on a clean shutdown. Snapshots are written to a
                local directory; nothing leaves your machine.
            </p>

            {#if backupStats}
                <dl class="backup-facts">
                    <dt>Snapshots</dt>
                    <dd>
                        {backupStats.count}
                        {#if backupStats.count > 0}
                            · {formatBytes(backupStats.total_bytes)} on disk
                        {/if}
                    </dd>
                    <dt>Last snapshot</dt>
                    <dd>
                        {backupStats.latest_at
                            ? formatTimestamp(
                                  backupStats.latest_at,
                                  tsFormat,
                              )
                            : '—'}
                    </dd>
                    <dt>Folder</dt>
                    <dd class="backup-path">
                        <code>{backupStats.directory}</code>
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
                    bind:value={backupRetention}
                />
            </label>
            <div class="actions">
                <button
                    type="button"
                    onclick={saveBackupRetention}
                    disabled={!backupRetentionDirty ||
                        backupRetentionStatus === 'saving'}
                >
                    {#if backupRetentionStatus === 'saving'}Saving…{:else}Save retention{/if}
                </button>
                {#if backupRetentionStatus === 'saved'}
                    <span class="hint">Saved.</span>
                {:else if backupRetentionDirty}
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
                    bind:value={backupIntervalHours}
                />
            </label>
            <div class="actions">
                <button
                    type="button"
                    onclick={saveBackupInterval}
                    disabled={!backupIntervalDirty ||
                        backupIntervalStatus === 'saving'}
                >
                    {#if backupIntervalStatus === 'saving'}Saving…{:else}Save interval{/if}
                </button>
                {#if backupIntervalStatus === 'saved'}
                    <span class="hint">Saved.</span>
                {:else if backupIntervalDirty}
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

        <section class="blocked-section">
            <details>
                <summary>
                    <span class="summary-title">
                        Blocked games{blocked.size > 0
                            ? ` (${blocked.size})`
                            : ''}
                    </span>
                </summary>
                <p class="description">
                    Blocked games stop recording new sessions and are hidden
                    from the Games and Recent views. Their history stays in
                    the database — unblock here to see them again.
                </p>
                {#if apps.length === 0}
                    <p class="hint">No applications tracked yet.</p>
                {:else}
                    <label class="search">
                        <span class="visually-hidden">Filter games</span>
                        <input
                            type="search"
                            placeholder="Filter by name or publisher…"
                            bind:value={filterQuery}
                        />
                    </label>
                {#if visibleApps.length === 0}
                    <p class="hint">No games match "{filterQuery}".</p>
                {:else}
                    <ul class="apps">
                        {#each visibleApps as app (app.id)}
                            {@const isBlocked = blocked.has(app.id)}
                            <li class:blocked={isBlocked}>
                                <div class="app-name">
                                    <span class="product">{app.product_name}</span>
                                    {#if app.publisher}
                                        <span class="publisher">{app.publisher}</span>
                                    {/if}
                                </div>
                                <button
                                    type="button"
                                    class="block-toggle"
                                    class:is-blocked={isBlocked}
                                    onclick={() => toggleBlock(app.id)}
                                >
                                    {isBlocked ? 'Unblock' : 'Block'}
                                </button>
                            </li>
                        {/each}
                    </ul>
                {/if}
                {/if}
            </details>
        </section>

        <section class="about">
            <h2>About</h2>
            <p class="about-tagline">Linux gameplay time tracker.</p>
            <dl class="about-facts">
                <dt>Version</dt>
                <dd>{appVersion || '—'}</dd>
                <dt>License</dt>
                <dd>MIT OR Apache-2.0</dd>
                <dt>Repository</dt>
                <dd>
                    <button
                        type="button"
                        class="link-button"
                        onclick={openRepo}
                    >
                        {REPO_URL}
                    </button>
                </dd>
            </dl>
            <p class="about-privacy">
                No telemetry. No network egress. Data stays under
                <code>$XDG_DATA_HOME/ludex/</code>.
            </p>
        </section>
    {/if}
</main>

<style>
    main {
        max-width: 80ch;
        margin: 0 auto;
        padding: 2rem;
    }

    header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 1.5rem;
    }

    h1 {
        font-size: 1.75rem;
        font-weight: 600;
        margin: 0;
        letter-spacing: -0.02em;
    }

    h2 {
        font-size: 1rem;
        font-weight: 600;
        color: var(--text-label);
        margin: 0 0 0.5rem;
    }

    section {
        background: var(--bg-surface);
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 1.25rem 1.5rem;
        margin-bottom: 1rem;
    }

    .about h2 {
        margin-bottom: 0.75rem;
    }

    .about-tagline {
        color: var(--text-secondary);
        margin: 0 0 1rem;
    }

    .about-facts {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 0.25rem 1rem;
        margin: 0 0 1rem;
        font-size: 0.88rem;
    }

    .about-facts dt {
        color: var(--text-subtle);
        text-transform: uppercase;
        font-size: 0.75rem;
        letter-spacing: 0.03em;
        align-self: center;
    }

    .about-facts dd {
        margin: 0;
        color: var(--text-secondary);
    }

    .link-button {
        background: none;
        border: none;
        padding: 0;
        color: var(--accent);
        font: inherit;
        cursor: pointer;
        text-align: left;
    }

    .link-button:hover {
        text-decoration: underline;
    }

    .about-privacy {
        color: var(--text-muted);
        font-size: 0.82rem;
        margin: 0;
        line-height: 1.5;
    }

    .about-privacy code {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        background: var(--code-bg);
        color: var(--code-text);
        padding: 0.1rem 0.35rem;
        border-radius: 4px;
        font-size: 0.8rem;
    }

    /* Collapsed section for the (potentially long) blocked-games
       list. Native <details> keeps a11y and keyboard support; we
       only restyle the summary so it reads like the other <h2>s. */
    .blocked-section summary {
        cursor: pointer;
        list-style: none;
        display: flex;
        align-items: center;
        gap: 0.5rem;
        user-select: none;
    }

    .blocked-section summary::-webkit-details-marker {
        display: none;
    }

    .blocked-section summary::before {
        content: '▸';
        color: var(--text-subtle);
        font-size: 0.75rem;
        transition: transform 120ms ease;
    }

    .blocked-section details[open] > summary::before {
        transform: rotate(90deg);
    }

    .blocked-section .summary-title {
        font-size: 1rem;
        font-weight: 600;
        color: var(--text-label);
    }

    .blocked-section details[open] > summary {
        margin-bottom: 0.75rem;
    }

    .description {
        color: var(--text-muted);
        font-size: 0.88rem;
        margin: 0 0 1rem;
        line-height: 1.5;
    }

    /* A second `.description` block within the same card. The
       spacing-and-divider keeps the two grace fields visually
       distinct so users don't conflate them. */
    .sub-description {
        margin-top: 1.5rem;
        padding-top: 1rem;
        border-top: 1px solid var(--border-soft);
    }

    .sub-description code {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        background: var(--code-bg);
        color: var(--code-text);
        padding: 0.05rem 0.3rem;
        border-radius: 4px;
        font-size: 0.78rem;
    }

    .field {
        display: flex;
        flex-direction: column;
        gap: 0.35rem;
        max-width: 18rem;
    }

    .field-label {
        font-size: 0.82rem;
        color: var(--text-label);
    }

    input[type='number'],
    input[type='search'],
    select {
        font: inherit;
        padding: 0.45rem 0.6rem;
        border: 1px solid var(--button-border);
        background: var(--bg-surface);
        color: var(--text-primary);
        border-radius: 6px;
        font-variant-numeric: tabular-nums;
    }

    input[type='number']:focus,
    input[type='search']:focus,
    select:focus {
        outline: 2px solid var(--accent);
        outline-offset: -1px;
    }

    input[type='number']:disabled {
        opacity: 0.55;
        cursor: not-allowed;
    }

    .toggle {
        display: flex;
        align-items: center;
        gap: 0.6rem;
        margin-bottom: 1rem;
        font-size: 0.88rem;
        color: var(--text-body);
        cursor: pointer;
    }

    .toggle input[type='checkbox'] {
        margin: 0;
        accent-color: var(--accent);
    }

    .search {
        display: block;
        margin-bottom: 0.75rem;
    }

    .search input {
        width: 100%;
        box-sizing: border-box;
    }

    .visually-hidden {
        position: absolute;
        width: 1px;
        height: 1px;
        padding: 0;
        margin: -1px;
        overflow: hidden;
        clip: rect(0, 0, 0, 0);
        white-space: nowrap;
        border: 0;
    }

    .actions {
        display: flex;
        align-items: center;
        gap: 0.75rem;
        margin-top: 0.75rem;
    }

    .apps {
        list-style: none;
        padding: 0;
        margin: 0;
    }

    .apps li {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 1rem;
        padding: 0.6rem 0;
        border-bottom: 1px solid var(--border-soft);
    }

    .apps li:last-child {
        border-bottom: none;
    }

    .apps li.blocked .product {
        color: var(--text-muted);
        text-decoration: line-through;
        text-decoration-thickness: 1px;
    }

    .app-name {
        display: flex;
        flex-direction: column;
        gap: 0.1rem;
        min-width: 0;
    }

    .product {
        color: var(--text-primary);
        font-size: 0.95rem;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .publisher {
        color: var(--text-muted);
        font-size: 0.8rem;
    }

    .block-toggle {
        min-width: 5.5rem;
    }

    .block-toggle.is-blocked {
        border-color: var(--accent);
        color: var(--accent);
    }

    /* Inline variant of the global `.error` block, used for
       per-action failures so the form stays visible. Pairs the
       message with a Dismiss button on the right. */
    .error.inline {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 1rem;
        padding: 0.6rem 0.9rem;
        margin-bottom: 1rem;
    }

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
