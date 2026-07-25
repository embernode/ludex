<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        listRecentSessions,
        onDaemonDisconnected,
        onDaemonReconnected,
        onSessionEnded,
        onSessionStarted,
        type SessionSummary,
    } from '$lib/api';
    import { formatSeconds } from '$lib/format';

    /** Matches the daemon's heartbeat cadence for open sessions. */
    const HEARTBEAT_MS = 60_000;

    /**
     * The open session, or `null` when nothing is being played. The
     * newest row is enough to answer this: the daemon holds a partial
     * unique index over open sessions, so at most one exists, and it
     * necessarily sorts first. This is the same reconcile the tray
     * does on reconnect.
     */
    let session = $state<SessionSummary | null>(null);
    let fetchedAt = $state<number>(Date.now());
    let now = $state<number>(Date.now());

    async function refresh() {
        try {
            const [newest] = await listRecentSessions(1);
            session = newest && !newest.ended_at ? newest : null;
            fetchedAt = Date.now();
            now = fetchedAt;
        } catch (_) {
            // The pill is ambient chrome; every view already reports
            // its own load failures, and a second banner for the same
            // dead daemon would be noise. Absence reads as "nothing
            // playing", which is also what a dead daemon means.
            session = null;
        }
    }

    /**
     * Seeded from the daemon's `full_runtime_seconds`, which it
     * measures against a monotonic clock so an NTP step or a suspend
     * can't distort it. Only the sub-heartbeat delta is added locally.
     */
    const elapsed = $derived.by(() => {
        if (!session) return '';
        return formatSeconds(
            session.full_runtime_seconds + Math.max(0, (now - fetchedAt) / 1000),
        );
    });

    $effect(() => {
        if (!session) return;
        const tick = setInterval(() => (now = Date.now()), 1000);
        const resync = setInterval(refresh, HEARTBEAT_MS);
        return () => {
            clearInterval(tick);
            clearInterval(resync);
        };
    });

    onMount(() => {
        refresh();
        const unlistens: Promise<UnlistenFn>[] = [
            onSessionStarted(refresh),
            onSessionEnded(refresh),
            onDaemonReconnected(refresh),
            onDaemonDisconnected(() => (session = null)),
        ];
        return () => {
            for (const p of unlistens) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

{#if session}
    <a class="pill" href="/app/{session.application_id}">
        <span class="dot"></span>
        <span class="label">{session.product_name} · {elapsed}</span>
    </a>
{/if}

<style>
    .pill {
        display: flex;
        align-items: center;
        gap: 7px;
        padding: 5px 11px 5px 9px;
        border-radius: 999px;
        border: 1px solid var(--pill-bd);
        background: var(--pill-bg);
        text-decoration: none;
        flex: none;
        max-width: 320px;
    }

    .dot {
        width: 7px;
        height: 7px;
        border-radius: 99px;
        background: var(--ac);
        flex: none;
    }

    .label {
        font-size: 12px;
        font-weight: 500;
        color: var(--pill-fg);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
</style>
