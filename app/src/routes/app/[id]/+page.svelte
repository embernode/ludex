<script lang="ts">
    import { onMount } from 'svelte';
    import { page } from '$app/state';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import ConfirmDialog from '$lib/ConfirmDialog.svelte';
    import {
        deleteSession,
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
        interactiveShare,
        observeTimestampFormat,
        type TimestampFormat,
    } from '$lib/format';
    import MonogramTile from '$lib/MonogramTile.svelte';

    /** How many of the newest session rows this view fetches. */
    const SESSION_LIMIT = 100;

    let app = $state<ApplicationSummary | null>(null);
    let sessions = $state<SessionSummary[]>([]);
    let loading = $state(true);
    let error = $state<string | null>(null);
    let tsFormat = $state<TimestampFormat>('short');

    // Route param is a string; convert once per navigation.
    const id = $derived(Number(page.params.id));

    /**
     * ProtonDB destination for this application. Steam-launched
     * games with a numeric appid go directly to the app's
     * compatibility-report page; everything else (non-Steam games,
     * Lutris-managed wine titles, Battle.net curated entries)
     * falls back to a name-keyed search since ProtonDB indexes
     * non-Steam titles too. `null` only when the product name is
     * empty — at which point a search would land on a useless
     * results page.
     */
    const protondb = $derived.by(() => {
        if (!app) return null;
        if (app.launcher_type === 'steam') {
            const appid = app.launcher_id.trim();
            if (appid && /^\d+$/.test(appid)) {
                return {
                    url: `https://www.protondb.com/app/${appid}`,
                    label: 'View compatibility report ↗',
                };
            }
        }
        const query = app.product_name.trim();
        if (!query) return null;
        return {
            url: `https://www.protondb.com/search?q=${encodeURIComponent(query)}`,
            label: 'Search compatibility reports ↗',
        };
    });

    async function openProtondb() {
        if (!protondb) return;
        try {
            await openUrl(protondb.url);
        } catch (e) {
            error = String(e);
        }
    }

    async function refresh() {
        if (!Number.isFinite(id) || id <= 0) {
            // Clear first: the error renders as an inline banner above
            // the content, so leaving the previous game's rows in place
            // would attribute them to an id that doesn't exist.
            app = null;
            sessions = [];
            error = `Invalid application id: ${page.params.id}`;
            loading = false;
            return;
        }
        try {
            const [results, sess] = await Promise.all([
                getApplication(id),
                listSessionsForApplication(id, SESSION_LIMIT),
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

    /**
     * The open session, if this game is being played right now.
     * `ended_at` is empty for exactly one row at most — the daemon
     * holds a partial unique index that guarantees it.
     */
    const openSession = $derived(sessions.find((s) => !s.ended_at) ?? null);

    /**
     * How often the daemon heartbeats an open session's runtime to the
     * database. Re-fetching on the same cadence keeps the pill anchored
     * to the daemon's own number.
     */
    const HEARTBEAT_MS = 60_000;

    // The live elapsed pill lives in the header, where it is visible
    // from every route; a second copy on the game's own page would
    // show the same session twice with two independently-fetched
    // counters that can disagree by up to a heartbeat. What this page
    // still needs from `openSession` is a reason to re-poll, so the
    // FULL column for an in-progress session doesn't sit stale.
    $effect(() => {
        if (!openSession) return;
        const resync = setInterval(refresh, HEARTBEAT_MS);
        return () => clearInterval(resync);
    });

    /**
     * Raw session rows behind the merged spans on screen. The daemon
     * applies `SESSION_LIMIT` to rows *before* folding adjacent
     * fragments, so the returned array is shorter than the limit
     * whenever anything merged — comparing its length against the
     * limit would hide the truncation notice in the common case.
     */
    const rawRowCount = $derived(
        sessions.reduce((n, s) => n + s.fragment_ids.length, 0),
    );

    /** Idle time is the part of full runtime that wasn't interactive. */
    const idleSeconds = $derived(
        app ? Math.max(0, app.total_full_seconds - app.total_interactive_seconds) : 0,
    );

    function statusLabel(s: SessionSummary): string {
        const base = s.exit_reason ? s.exit_reason.replace(/_/g, ' ') : 'open';
        if (s.fragment_ids.length > 1) {
            return `${base} · ${s.fragment_ids.length} merged`;
        }
        return base;
    }

    /**
     * Whether this session row is safe to delete from the GUI.
     * Open sessions belong to the daemon and would lose in-flight
     * runtime if dropped — those still bail out. Merged spans are
     * fine to delete: we hand the daemon the span's `fragment_ids`
     * and it removes exactly those rows in one transaction, and
     * the confirm dialog tells the user how many underlying rows
     * are about to go.
     */
    function canDelete(s: SessionSummary): boolean {
        return Boolean(s.exit_reason);
    }

    /** Underlying `<dialog>` element, owned by the `ConfirmDialog`
     *  component but threaded back here via two-way binding so we
     *  can call `showModal()` / `close()` imperatively. */
    let deleteDialog = $state<HTMLDialogElement | null>(null);
    /** Session queued for deletion while the dialog is open. */
    let pendingDelete = $state<SessionSummary | null>(null);

    function openDeleteDialog(s: SessionSummary) {
        pendingDelete = s;
        deleteDialog?.showModal();
    }

    async function performDelete() {
        if (!pendingDelete) return;
        try {
            await deleteSession(pendingDelete.fragment_ids);
            deleteDialog?.close();
            pendingDelete = null;
            // Refresh both the session list and the application
            // stats card; both move when a row is removed.
            await refresh();
        } catch (e) {
            error = String(e);
            // Re-throw so the dialog catches it and clears its
            // own busy state — the dialog stays open so the user
            // sees the error banner above and can dismiss with
            // Cancel/ESC.
            throw e;
        }
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
    <a class="back" href="/">← Library</a>

    {#if loading && !app}
        <p class="state">Loading…</p>
    {:else if error && !app}
        <div class="error">
            <p><strong>Couldn't load this game.</strong></p>
            <p class="detail">{error}</p>
        </div>
    {:else if !app}
        <div class="empty">
            <p>No such application.</p>
            <p class="hint"><a href="/">Back to the library</a></p>
        </div>
    {:else}
        {#if error}
            <div class="error inline">
                <p class="detail">{error}</p>
                <button
                    type="button"
                    class="link-button"
                    onclick={() => (error = null)}
                >
                    Dismiss
                </button>
            </div>
        {/if}

        <div class="identity">
            <MonogramTile name={app.product_name} size={52} />
            <div class="titles">
                <h1>{app.product_name}</h1>
                <div class="chips">
                    <span class="keychip mono">
                        {app.launcher_type}:{app.launcher_id}
                    </span>
                    {#if protondb}
                        <button
                            type="button"
                            class="protondb"
                            onclick={openProtondb}
                        >
                            {protondb.label}
                        </button>
                    {/if}
                </div>
            </div>
        </div>

        <div class="statstrip">
            <div class="cell runtime">
                <div class="celllabel">RUNTIME</div>
                <div class="bigline">
                    <span class="num big">
                        {formatSeconds(app.total_full_seconds)}
                    </span>
                    <span class="unit">full</span>
                </div>
                <div class="bar">
                    <span
                        style="width:{interactiveShare(
                            app.total_interactive_seconds,
                            app.total_full_seconds,
                        )}%"
                    ></span>
                </div>
                <div class="subline">
                    <span class="num">
                        {formatSeconds(app.total_interactive_seconds)} interactive
                    </span>
                    {#if idleSeconds > 0}
                        <span class="dim">
                            · {formatSeconds(idleSeconds)} idle subtracted
                        </span>
                    {/if}
                </div>
            </div>
            <div class="cell">
                <div class="celllabel">SESSIONS</div>
                <div class="num big">{app.run_count}</div>
            </div>
            <div class="cell">
                <div class="celllabel">FIRST SEEN</div>
                <div class="num medium">
                    {formatTimestamp(app.first_seen_at, tsFormat)}
                </div>
            </div>
            <div class="cell">
                <div class="celllabel">LONGEST</div>
                <div class="num big">
                    {formatSeconds(app.longest_full_seconds)}
                </div>
            </div>
        </div>

        <div class="sessionhead">
            <h2>Sessions</h2>
            {#if rawRowCount >= SESSION_LIMIT}
                <span class="dim">newest {SESSION_LIMIT}</span>
            {/if}
        </div>

        {#if sessions.length === 0}
            <p class="state">No sessions recorded yet.</p>
        {:else}
            <!-- Laid out with grid rather than <table> to match the
                 design, so the table roles are restated explicitly —
                 without them a screen reader reads the header line
                 once as prose and then each row as unlabelled values. -->
            <div class="tablewrap" role="table" aria-label="Sessions">
                <div class="grid thead" role="row">
                    <span role="columnheader">DATE</span>
                    <span role="columnheader">ENDED</span>
                    <span class="right" role="columnheader">FULL</span>
                    <span role="columnheader">INTERACTIVE</span>
                    <span role="columnheader">OUTCOME</span>
                    <span role="columnheader"><span class="visually-hidden">Actions</span></span>
            </div>
            <ul class="rows" role="rowgroup">
                {#each sessions as s (s.id)}
                    <li class="grid row" role="row">
                        <span class="num dim" role="cell">
                            {formatTimestamp(s.started_at, tsFormat)}
                        </span>
                        <span class="mono num dim" role="cell">
                            {formatTimestamp(s.ended_at, tsFormat)}
                        </span>
                        <span class="right num strong" role="cell">
                            {formatSeconds(s.full_runtime_seconds)}
                        </span>
                        <span class="interactive" role="cell">
                            <span class="bar">
                                <span
                                    style="width:{interactiveShare(
                                        s.interactive_runtime_seconds,
                                        s.full_runtime_seconds,
                                    )}%"
                                ></span>
                            </span>
                            <span class="mono num dim">
                                {formatSeconds(s.interactive_runtime_seconds)}
                            </span>
                        </span>
                        <span class="status" class:open={!s.exit_reason} role="cell">
                            {statusLabel(s)}
                        </span>
                        <span class="rowaction" role="cell">
                            {#if canDelete(s)}
                                <button
                                    type="button"
                                    class="delete"
                                    title="Delete this session"
                                    aria-label="Delete the session starting {formatTimestamp(
                                        s.started_at,
                                        tsFormat,
                                    )}"
                                    onclick={() => openDeleteDialog(s)}
                                >
                                    ✕
                                </button>
                            {/if}
                        </span>
                    </li>
                {/each}
            </ul>
            </div>
        {/if}
    {/if}

    <ConfirmDialog
        bind:dialog={deleteDialog}
        title="Delete this session?"
        confirmLabel="Delete session"
        confirmBusyLabel="Deleting…"
        danger
        onconfirm={performDelete}
    >
        {#snippet body()}
            {#if pendingDelete}
                <dl class="confirm-facts">
                    <dt>Started</dt>
                    <dd>{formatTimestamp(pendingDelete.started_at, tsFormat)}</dd>
                    <dt>Ended</dt>
                    <dd>{formatTimestamp(pendingDelete.ended_at, tsFormat)}</dd>
                    <dt>Full</dt>
                    <dd>{formatSeconds(pendingDelete.full_runtime_seconds)}</dd>
                    <dt>Interactive</dt>
                    <dd>
                        {formatSeconds(
                            pendingDelete.interactive_runtime_seconds,
                        )}
                    </dd>
                </dl>
                {#if pendingDelete.fragment_ids.length > 1}
                    <p class="confirm-warning">
                        This row is a merged span;
                        <strong>all {pendingDelete.fragment_ids.length} underlying sessions</strong>
                        will be removed.
                    </p>
                {/if}
                <p class="confirm-warning">
                    This cannot be undone. Aggregate stats for
                    <strong>{app?.product_name}</strong> are recomputed
                    from the surviving sessions.
                </p>
            {/if}
        {/snippet}
    </ConfirmDialog>
</main>

<style>
    main {
        max-width: 900px;
        margin: 0 auto;
        padding: 22px 20px 40px;
    }

    .back {
        display: inline-block;
        font-size: 12.5px;
        color: var(--fg3);
        text-decoration: none;
        margin-bottom: 16px;
    }

    .back:hover {
        color: var(--fg);
    }

    .identity {
        display: flex;
        align-items: flex-start;
        gap: 14px;
        margin-bottom: 18px;
    }

    .titles {
        flex: 1;
        min-width: 0;
    }

    h1 {
        font-size: 26px;
        font-weight: 600;
        line-height: 1;
        margin: 0 0 7px;
        letter-spacing: -0.02em;
    }

    h2 {
        font-size: 15px;
        font-weight: 600;
        margin: 0;
    }

    .chips {
        display: flex;
        align-items: center;
        gap: 8px;
        flex-wrap: wrap;
    }

    .keychip {
        font-size: 11.5px;
        color: var(--fg2);
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 5px;
        padding: 3px 8px;
    }

    .protondb {
        font: inherit;
        font-size: 11.5px;
        color: var(--ac);
        background: none;
        border: 0;
        padding: 0;
        cursor: pointer;
    }

    .protondb:hover {
        background: none;
        text-decoration: underline;
    }

    .statstrip {
        display: flex;
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 9px;
        overflow: hidden;
        margin-bottom: 20px;
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

    .runtime {
        flex: 2;
    }

    .celllabel {
        font-size: 10.5px;
        font-weight: 500;
        letter-spacing: 0.09em;
        color: var(--fg3);
        margin-bottom: 8px;
    }

    .bigline {
        display: flex;
        align-items: baseline;
        gap: 8px;
        white-space: nowrap;
    }

    .big {
        font-size: 21px;
        font-weight: 600;
        line-height: 1;
    }

    .medium {
        font-size: 15px;
        font-weight: 600;
        line-height: 1.2;
    }

    .unit {
        font-size: 12px;
        color: var(--fg3);
    }

    .subline {
        display: flex;
        align-items: baseline;
        gap: 6px;
        margin-top: 6px;
        white-space: nowrap;
        font-size: 12px;
        font-weight: 500;
        color: var(--fg2);
        flex-wrap: wrap;
    }

    .runtime .bar {
        margin-top: 8px;
        height: 6px;
    }

    .bar {
        flex: 1;
        height: 5px;
        border-radius: 99px;
        overflow: hidden;
        background: var(--track);
    }

    .bar > span {
        display: block;
        height: 100%;
        background: var(--ac);
    }

    .sessionhead {
        display: flex;
        align-items: baseline;
        gap: 10px;
        margin-bottom: 9px;
    }

    .grid {
        display: grid;
        grid-template-columns:
            150px 150px 88px 168px minmax(120px, 1fr) 30px;
        gap: 12px;
        align-items: center;
    }

    /* Same surface as the library and detections tables. */
    .tablewrap {
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 9px;
        overflow: hidden;
    }

    .thead {
        padding: 12px 12px 8px;
        border-bottom: 1px solid var(--line);
        font-size: 10.5px;
        font-weight: 500;
        letter-spacing: 0.09em;
        color: var(--fg3);
    }

    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
    }

    .row {
        padding: 10px 12px;
        border-bottom: 1px solid var(--hair);
    }

    .rows li:last-child.row {
        border-bottom: 0;
    }

    .right {
        text-align: right;
    }

    .num {
        font-variant-numeric: tabular-nums;
    }

    .mono {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
    }

    .dim {
        font-size: 12px;
        color: var(--fg2);
    }

    .strong {
        font-size: 12.5px;
        font-weight: 500;
    }

    .interactive {
        display: flex;
        align-items: center;
        gap: 9px;
        min-width: 0;
    }

    .status {
        font-size: 11.5px;
        color: var(--fg2);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .status.open {
        color: var(--ac);
    }

    .rowaction {
        text-align: center;
    }

    .delete {
        font: inherit;
        font-size: 12px;
        color: var(--fg3);
        background: none;
        border: 0;
        padding: 2px 4px;
        cursor: pointer;
        border-radius: 4px;
    }

    .delete:focus-visible {
        outline: 2px solid var(--ac);
        outline-offset: -1px;
    }

    .delete:hover {
        color: var(--warn);
        background: none;
    }

    .state {
        font-size: 12.5px;
        color: var(--fg3);
        padding: 20px 12px;
        margin: 0;
    }

    /* Body content rendered into the ConfirmDialog component's `body`
       snippet. The component owns the dialog frame / buttons /
       ::backdrop; the parent supplies the content and its styling. */
    .confirm-facts {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 0.3rem 1rem;
        margin: 0 0 1rem;
        font-size: 0.85rem;
    }

    .confirm-facts dt {
        color: var(--fg3);
    }

    .confirm-facts dd {
        margin: 0;
        color: var(--fg2);
    }

    .confirm-warning {
        font-size: 0.85rem;
        color: var(--fg2);
        margin: 0 0 0.5rem;
        line-height: 1.5;
    }
</style>
