<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        listBlockedApplicationIds,
        listRecentSessions,
        onDaemonReconnected,
        onSessionEnded,
        onSessionStarted,
        type SessionSummary,
    } from '$lib/api';
    import { formatSeconds, formatTimestamp } from '$lib/format';

    let sessions = $state<SessionSummary[]>([]);
    let hiddenBlocked = $state(0);
    let loading = $state(true);
    let error = $state<string | null>(null);

    async function refresh() {
        try {
            const [recent, blockedIds] = await Promise.all([
                listRecentSessions(100),
                listBlockedApplicationIds().catch(() => [] as number[]),
            ]);
            const blocked = new Set(blockedIds);
            sessions = recent.filter((s) => !blocked.has(s.application_id));
            hiddenBlocked = recent.length - sessions.length;
            error = null;
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
        }
    }

    function statusLabel(s: SessionSummary): string {
        if (!s.exit_reason) return 'open';
        return s.exit_reason.replace(/_/g, ' ');
    }

    onMount(() => {
        refresh();
        const unlisteners: Promise<UnlistenFn>[] = [
            onSessionStarted(refresh),
            onSessionEnded(refresh),
            onDaemonReconnected(refresh),
        ];
        return () => {
            for (const p of unlisteners) {
                p.then((unlisten) => unlisten()).catch(() => {});
            }
        };
    });
</script>

<main>
    <header>
        <h1>Recent sessions</h1>
        <button onclick={refresh} disabled={loading}>Refresh</button>
    </header>

    {#if loading && sessions.length === 0}
        <p class="hint">Loading…</p>
    {:else if error}
        <div class="error">
            <p><strong>Couldn't reach the daemon.</strong></p>
            <p class="detail">{error}</p>
            <p class="hint">Is <code>ludex-daemon</code> running?</p>
        </div>
    {:else if sessions.length === 0 && hiddenBlocked === 0}
        <div class="empty">
            <p>No sessions yet.</p>
            <p class="hint">Sessions appear here as soon as a game starts.</p>
        </div>
    {:else if sessions.length === 0}
        <div class="empty">
            <p>Nothing to show — every recent session belongs to a blocked game.</p>
            <p class="hint">
                Unblock from <a href="/settings">Settings</a> to see these again.
            </p>
        </div>
    {:else}
        <table>
            <thead>
                <tr>
                    <th>Started</th>
                    <th>Game</th>
                    <th>Full</th>
                    <th>Interactive</th>
                    <th>Status</th>
                </tr>
            </thead>
            <tbody>
                {#each sessions as s (s.id)}
                    <tr>
                        <td>{formatTimestamp(s.started_at)}</td>
                        <td
                            ><a href="/app/{s.application_id}"
                                >{s.product_name}</a
                            ></td
                        >
                        <td class="num">{formatSeconds(s.full_runtime_seconds)}</td>
                        <td class="num"
                            >{formatSeconds(s.interactive_runtime_seconds)}</td
                        >
                        <td class="status" class:open={!s.exit_reason}
                            >{statusLabel(s)}</td
                        >
                    </tr>
                {/each}
            </tbody>
        </table>
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

    table {
        width: 100%;
        border-collapse: collapse;
        background: var(--bg-surface);
        border: 1px solid var(--border);
        border-radius: 8px;
        overflow: hidden;
    }

    th,
    td {
        padding: 0.6rem 0.9rem;
        text-align: left;
        font-size: 0.9rem;
    }

    th {
        background: var(--bg-hover);
        color: var(--text-muted);
        font-weight: 500;
        font-size: 0.75rem;
        text-transform: uppercase;
        letter-spacing: 0.03em;
        border-bottom: 1px solid var(--border);
    }

    tbody tr {
        border-bottom: 1px solid var(--border-soft);
    }

    tbody tr:last-child {
        border-bottom: none;
    }

    tbody tr:hover {
        background: var(--bg-hover);
    }

    .num {
        font-variant-numeric: tabular-nums;
        color: var(--text-secondary);
    }

    .status {
        color: var(--text-muted);
        font-size: 0.85rem;
    }

    .status.open {
        color: var(--status-open);
        font-weight: 500;
    }

    a {
        color: var(--accent);
        text-decoration: none;
    }

    a:hover {
        text-decoration: underline;
    }
</style>
