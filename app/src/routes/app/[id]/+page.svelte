<script lang="ts">
    import { onMount } from 'svelte';
    import { page } from '$app/state';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        getApplication,
        listSessionsForApplication,
        onDaemonReconnected,
        onSessionEnded,
        onSessionStarted,
        type ApplicationSummary,
        type SessionSummary,
    } from '$lib/api';
    import {
        formatSeconds,
        formatTimestamp,
        observeTimestampFormat,
        type TimestampFormat,
    } from '$lib/format';

    let app = $state<ApplicationSummary | null>(null);
    let sessions = $state<SessionSummary[]>([]);
    let loading = $state(true);
    let error = $state<string | null>(null);
    let tsFormat = $state<TimestampFormat>('short');

    // Route param is a string; convert once per navigation.
    const id = $derived(Number(page.params.id));

    async function refresh() {
        if (!Number.isFinite(id) || id <= 0) {
            error = `Invalid application id: ${page.params.id}`;
            loading = false;
            return;
        }
        try {
            const [results, sess] = await Promise.all([
                getApplication(id),
                listSessionsForApplication(id, 100),
            ]);
            app = results[0] ?? null;
            sessions = sess;
            error = null;
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
        }
    }

    function statusLabel(s: SessionSummary): string {
        const base = s.exit_reason ? s.exit_reason.replace(/_/g, ' ') : 'open';
        if (s.fragment_count > 1) {
            return `${base} · ${s.fragment_count} merged`;
        }
        return base;
    }

    // Re-fetch when the route id changes. `$effect` replaces Svelte 4's
    // reactive blocks; it re-runs whenever anything it reads changes.
    $effect(() => {
        // Reading `id` is what triggers the re-run.
        void id;
        refresh();
    });

    onMount(() => {
        const unobserveTs = observeTimestampFormat((f) => (tsFormat = f));
        const unlisteners: Promise<UnlistenFn>[] = [
            onSessionStarted(refresh),
            onSessionEnded(refresh),
            onDaemonReconnected(refresh),
        ];
        return () => {
            unobserveTs();
            for (const p of unlisteners) {
                p.then((unlisten) => unlisten()).catch(() => {});
            }
        };
    });
</script>

<main>
    <nav class="crumb">
        <a href="/">← Games</a>
    </nav>

    {#if loading && !app}
        <p class="hint">Loading…</p>
    {:else if error}
        <div class="error">
            <p><strong>Couldn't load this application.</strong></p>
            <p class="detail">{error}</p>
        </div>
    {:else if !app}
        <div class="empty">
            <p>No application with id {id}.</p>
            <p class="hint">It may have been removed, or never existed.</p>
        </div>
    {:else}
        <header>
            <div class="title">
                <h1>{app.product_name}</h1>
                {#if app.publisher}
                    <span class="publisher">{app.publisher}</span>
                {/if}
                <span class="id-badge" title="Application id (for ludex merge)"
                    >#{app.id}</span
                >
            </div>
            <button onclick={refresh}>Refresh</button>
        </header>

        <section class="stats">
            <div class="stat-card">
                <div class="stat-label">Runs</div>
                <div class="stat-value">{app.run_count}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Full runtime</div>
                <div class="stat-value">{formatSeconds(app.total_full_seconds)}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Interactive</div>
                <div class="stat-value">
                    {formatSeconds(app.total_interactive_seconds)}
                </div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Last played</div>
                <div class="stat-value">
                    {formatTimestamp(app.last_played_at, tsFormat)}
                </div>
            </div>
        </section>

        <section class="identity">
            <h2>Identity</h2>
            <dl>
                <dt>Launcher</dt>
                <dd><code>{app.launcher_type}:{app.launcher_id}</code></dd>
            </dl>
        </section>

        <section class="sessions">
            <h2>Sessions</h2>
            {#if sessions.length === 0}
                <p class="hint">No sessions recorded for this application.</p>
            {:else}
                <table>
                    <thead>
                        <tr>
                            <th>Started</th>
                            <th>Ended</th>
                            <th>Full</th>
                            <th>Interactive</th>
                            <th>Status</th>
                        </tr>
                    </thead>
                    <tbody>
                        {#each sessions as s (s.id)}
                            <tr>
                                <td>{formatTimestamp(s.started_at, tsFormat)}</td>
                                <td>{formatTimestamp(s.ended_at, tsFormat)}</td>
                                <td class="num"
                                    >{formatSeconds(s.full_runtime_seconds)}</td
                                >
                                <td class="num"
                                    >{formatSeconds(
                                        s.interactive_runtime_seconds,
                                    )}</td
                                >
                                <td class="status" class:open={!s.exit_reason}
                                    >{statusLabel(s)}</td
                                >
                            </tr>
                        {/each}
                    </tbody>
                </table>
            {/if}
        </section>
    {/if}
</main>

<style>
    main {
        max-width: 80ch;
        margin: 0 auto;
        padding: 2rem;
    }

    .crumb {
        margin-bottom: 1rem;
    }

    .crumb a {
        color: var(--text-muted);
        text-decoration: none;
        font-size: 0.9rem;
    }

    .crumb a:hover {
        color: var(--text-primary);
    }

    header {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        margin-bottom: 1.5rem;
    }

    .title {
        display: flex;
        align-items: baseline;
        gap: 0.75rem;
        flex-wrap: wrap;
    }

    h1 {
        font-size: 1.75rem;
        font-weight: 600;
        margin: 0;
        letter-spacing: -0.02em;
    }

    h2 {
        font-size: 1.05rem;
        font-weight: 600;
        margin: 2rem 0 0.75rem;
        color: var(--text-label);
    }

    .publisher {
        font-size: 0.95rem;
        color: var(--text-muted);
    }

    .id-badge {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 0.78rem;
        color: var(--text-subtle);
        background: var(--tag-bg);
        padding: 0.1rem 0.45rem;
        border-radius: 999px;
        font-variant-numeric: tabular-nums;
        cursor: help;
    }

    .stats {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
        gap: 0.75rem;
    }

    .stat-card {
        background: var(--bg-surface);
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 0.9rem 1rem;
    }

    .stat-label {
        color: var(--text-subtle);
        font-size: 0.75rem;
        text-transform: uppercase;
        letter-spacing: 0.03em;
        margin-bottom: 0.25rem;
    }

    .stat-value {
        color: var(--text-primary);
        font-size: 1.15rem;
        font-weight: 600;
        font-variant-numeric: tabular-nums;
    }

    .identity dl {
        display: grid;
        grid-template-columns: auto 1fr;
        gap: 0.35rem 1rem;
        margin: 0;
    }

    .identity dt {
        color: var(--text-subtle);
        font-size: 0.85rem;
    }

    .identity dd {
        margin: 0;
        font-size: 0.9rem;
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
        padding: 0.55rem 0.85rem;
        text-align: left;
        font-size: 0.88rem;
    }

    th {
        background: var(--bg-hover);
        color: var(--text-muted);
        font-weight: 500;
        font-size: 0.72rem;
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
</style>
