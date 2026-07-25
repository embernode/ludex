// Typed wrappers around the Tauri invoke / event surface exposed
// by `src-tauri/src/bridge.rs`. This is the only module in the
// frontend that talks to the daemon; pages and components call
// through here rather than reaching for `@tauri-apps/api` directly.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/**
 * Whether we're running inside a Tauri webview. Detected at module
 * load via the presence of `__TAURI_INTERNALS__`, which Tauri
 * injects into `window` before any script runs. Outside Tauri — e.g.
 * `pnpm run dev` opened in a regular browser to bisect a styling
 * issue — we skip the event-listener wiring so the page loads
 * cleanly instead of logging `transformCallback` errors.
 */
const IN_TAURI =
    typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** `listen()` that returns a no-op unlisten when not in Tauri. */
function safeListen<T>(
    event: string,
    handler: (event: { payload: T }) => void,
): Promise<UnlistenFn> {
    if (!IN_TAURI) {
        return Promise.resolve(() => {});
    }
    return listen<T>(event, handler);
}

/** An application row sourced from `net.ludex.Tracker1.ListApplications`. */
export interface ApplicationSummary {
    id: number;
    launcher_type: string;
    launcher_id: string;
    product_name: string;
    /** Empty string when the publisher is unknown. */
    publisher: string;
    total_full_seconds: number;
    total_interactive_seconds: number;
    run_count: number;
    /** RFC 3339 UTC timestamp; empty string for never-played apps. */
    last_played_at: string;
}

/** A session row sourced from `net.ludex.Tracker1.ListRecentSessions`. */
export interface SessionSummary {
    id: number;
    application_id: number;
    product_name: string;
    started_at: string;
    /** Empty string while the session is still open. */
    ended_at: string;
    full_runtime_seconds: number;
    interactive_runtime_seconds: number;
    /** Empty string while the session is still open. */
    exit_reason: string;
    /**
     * Primary keys of the database rows folded into this summary,
     * newest first. A single element means an unmerged row; more
     * than one means the daemon collapsed consecutive same-application
     * sessions whose end-to-start gap was shorter than the merge
     * threshold (about a minute today). Time totals reflect the
     * merged span. Pass this whole array to `deleteSession` to drop
     * the span exactly as displayed.
     */
    fragment_ids: number[];
}

/** One day's aggregate runtime from `net.ludex.Tracker1.ListDailyPlaytime`. */
export interface DailyPlaytime {
    /** `YYYY-MM-DD` local calendar date (the daemon's timezone). */
    date: string;
    full_runtime_seconds: number;
    interactive_runtime_seconds: number;
    session_count: number;
}

/** Payload of the `ludex:session-ended` event. */
export interface SessionEndedPayload {
    application_id: number;
    full_runtime_seconds: number;
    interactive_runtime_seconds: number;
}

export async function listApplications(): Promise<ApplicationSummary[]> {
    return invoke<ApplicationSummary[]>('list_applications');
}

/**
 * Look up one application by id. D-Bus lacks a clean "optional"
 * primitive, so the daemon returns a 0-or-1-element array and we
 * expose the same shape.
 */
export async function getApplication(
    id: number,
): Promise<ApplicationSummary[]> {
    return invoke<ApplicationSummary[]>('get_application', { id });
}

export async function listRecentSessions(limit = 20): Promise<SessionSummary[]> {
    return invoke<SessionSummary[]>('list_recent_sessions', { limit });
}

/**
 * `invoke('list_daily_playtime', { days })` returns one row per
 * day with activity over the last `days` days, oldest first. Days
 * with no sessions are omitted; callers that need a continuous
 * axis fill zeros themselves.
 */
export async function listDailyPlaytime(days: number): Promise<DailyPlaytime[]> {
    return invoke<DailyPlaytime[]>('list_daily_playtime', { days });
}

export async function listSessionsForApplication(
    applicationId: number,
    limit = 50,
): Promise<SessionSummary[]> {
    return invoke<SessionSummary[]>('list_sessions_for_application', {
        applicationId,
        limit,
    });
}

/** Primary keys of every application the user has blocked. */
export async function listBlockedApplicationIds(): Promise<number[]> {
    return invoke<number[]>('list_blocked_application_ids');
}

/** Mark the application as blocked; the daemon drops future Started events for it. */
export async function blockApplication(id: number): Promise<void> {
    return invoke<void>('block_application', { id });
}

/** Remove the block so future Started events open sessions normally. */
export async function unblockApplication(id: number): Promise<void> {
    return invoke<void>('unblock_application', { id });
}

/**
 * Delete the given closed session rows and recompute the owning
 * application's denormalized stats. Resolves with `true` when at
 * least one row was removed, `false` when none matched (already
 * gone). The daemon refuses to delete open sessions and surfaces a
 * clear error string the GUI can display.
 *
 * Pass a session summary's `fragment_ids` to delete the whole
 * merged span exactly as displayed — the daemon drops precisely
 * these rows and never re-derives the span, so it can't reach
 * older fragments the user never saw (PERSIST-2).
 */
export async function deleteSession(ids: number[]): Promise<boolean> {
    return invoke<boolean>('delete_session', { ids });
}

/** Read the per-process GPU memory threshold (bytes) used by the gate. */
export async function getGpuMemoryThresholdBytes(): Promise<number> {
    return invoke<number>('get_gpu_memory_threshold_bytes');
}

/**
 * Persist the GPU memory threshold. The daemon applies the new
 * value in-process; the next foreground-window activation uses it.
 */
export async function setGpuMemoryThresholdBytes(bytes: number): Promise<void> {
    return invoke<void>('set_gpu_memory_threshold_bytes', { bytes });
}

/** Read the alt-tab grace window (seconds) used by the foreground source. */
export async function getAltTabGraceSeconds(): Promise<number> {
    return invoke<number>('get_alt_tab_grace_seconds');
}

/**
 * Persist the alt-tab grace window. Live-reloaded — the very next
 * grace timer uses the new value.
 */
export async function setAltTabGraceSeconds(seconds: number): Promise<void> {
    return invoke<void>('set_alt_tab_grace_seconds', { seconds });
}

/** Whether losing focus pauses the session. When false the session runs
 *  until the game process exits. */
export async function getPauseWhenBackgrounded(): Promise<boolean> {
    return invoke<boolean>('get_pause_when_backgrounded');
}

/** Persist the focus-pause toggle. Live-reloaded. */
export async function setPauseWhenBackgrounded(pause: boolean): Promise<void> {
    return invoke<void>('set_pause_when_backgrounded', { pause });
}

/**
 * Per-idle-interval cutscene grace (seconds). The first `grace`
 * seconds of every input-idle period are credited to interactive
 * runtime instead of subtracted as AFK; covers cutscenes, dialogue
 * trees, and similar engagement-without-input events.
 */
export async function getIdleGraceSeconds(): Promise<number> {
    return invoke<number>('get_idle_grace_seconds');
}

/** Persist the cutscene-grace window. Live-reloaded — applies to
 *  the next heartbeat / close-session calculation. */
export async function setIdleGraceSeconds(seconds: number): Promise<void> {
    return invoke<void>('set_idle_grace_seconds', { seconds });
}

/** Snapshot of the database-backup directory, served by the daemon. */
export interface BackupStats {
    /** Absolute path to the backup directory. */
    directory: string;
    /** Number of snapshots currently on disk. */
    count: number;
    /** Cumulative byte size across every snapshot. */
    total_bytes: number;
    /** RFC 3339 timestamp of the newest snapshot, empty when none. */
    latest_at: string;
}

/** Hours between automatic database snapshots. */
export async function getBackupIntervalHours(): Promise<number> {
    return invoke<number>('get_backup_interval_hours');
}

/**
 * Persist the backup interval (hours). Live-reloaded — the daemon's
 * scheduler resets its timer rather than waiting out the old period.
 */
export async function setBackupIntervalHours(hours: number): Promise<void> {
    return invoke<void>('set_backup_interval_hours', { hours });
}

/** Number of snapshots the daemon retains after each prune. */
export async function getBackupRetentionCount(): Promise<number> {
    return invoke<number>('get_backup_retention_count');
}

/** Persist the retention count. Applied on the next prune cycle. */
export async function setBackupRetentionCount(count: number): Promise<void> {
    return invoke<void>('set_backup_retention_count', { count });
}

/**
 * Ask the daemon to take a snapshot now and prune to the configured
 * retention. Resolves with the absolute path of the new snapshot.
 */
export async function takeBackupNow(): Promise<string> {
    return invoke<string>('take_backup_now');
}

/** Directory + size + last-snapshot summary the settings page shows. */
export async function getBackupStats(): Promise<BackupStats> {
    return invoke<BackupStats>('get_backup_stats');
}

/** Open the backup directory in the user's file manager. */
export async function openBackupDirectory(path: string): Promise<void> {
    return invoke<void>('open_backup_directory', { path });
}

export function onApplicationAdded(
    cb: (applicationId: number) => void,
): Promise<UnlistenFn> {
    return safeListen<number>('ludex:application-added', (event) => cb(event.payload));
}

export function onSessionStarted(
    cb: (applicationId: number) => void,
): Promise<UnlistenFn> {
    return safeListen<number>('ludex:session-started', (event) => cb(event.payload));
}

export function onSessionEnded(
    cb: (payload: SessionEndedPayload) => void,
): Promise<UnlistenFn> {
    return safeListen<SessionEndedPayload>('ludex:session-ended', (event) =>
        cb(event.payload),
    );
}

/**
 * Fires when the bridge rebuilds its D-Bus subscription — i.e. after
 * `ludex-daemon` came up or restarted. Subscribe alongside the
 * session-lifecycle handlers so pages re-fetch state that may have
 * changed while the daemon was down (merges, restores, imports).
 *
 * Not emitted on the first-ever connect: the page's own `onMount`
 * fetch already covers that case.
 */
export function onDaemonReconnected(cb: () => void): Promise<UnlistenFn> {
    return safeListen<null>('ludex:daemon-reconnected', () => cb());
}

/**
 * Fires when the bridge's D-Bus subscription to the daemon drops —
 * owner change, streams closed, or a failed re-subscribe. Pair with
 * `onDaemonReconnected` to show live connection state.
 */
export function onDaemonDisconnected(cb: () => void): Promise<UnlistenFn> {
    return safeListen<null>('ludex:daemon-disconnected', () => cb());
}

/**
 * Fires after a successful `blockApplication` / `unblockApplication`
 * call. Filtered views (Games, Recent, Dashboard) subscribe so the
 * blocklist change takes effect instantly instead of waiting for
 * the next session event.
 */
export function onBlocklistChanged(cb: () => void): Promise<UnlistenFn> {
    return safeListen<null>('ludex:blocklist-changed', () => cb());
}
