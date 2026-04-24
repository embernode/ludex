<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        blockApplication,
        getGpuMemoryThresholdBytes,
        listApplications,
        listBlockedApplicationIds,
        onDaemonReconnected,
        setGpuMemoryThresholdBytes,
        unblockApplication,
        type ApplicationSummary,
    } from '$lib/api';

    /** MiB <-> bytes (we show mebibytes in the UI). */
    const MIB = 1024 * 1024;

    let apps = $state<ApplicationSummary[]>([]);
    let blocked = $state<Set<number>>(new Set());
    let loading = $state(true);
    let error = $state<string | null>(null);

    /** Bytes currently persisted, for dirty-check. */
    let savedThresholdBytes = $state<number>(0);
    /** MiB, edit-in-progress value bound to the input. */
    let thresholdMib = $state<number>(50);
    let saveStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    async function load() {
        loading = true;
        try {
            const [a, ids, threshold] = await Promise.all([
                listApplications(),
                listBlockedApplicationIds(),
                getGpuMemoryThresholdBytes(),
            ]);
            apps = a;
            blocked = new Set(ids);
            savedThresholdBytes = threshold;
            thresholdMib = Math.max(1, Math.round(threshold / MIB));
            error = null;
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
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
        saveStatus = 'saving';
        try {
            await setGpuMemoryThresholdBytes(bytes);
            savedThresholdBytes = bytes;
            saveStatus = 'saved';
            setTimeout(() => {
                if (saveStatus === 'saved') saveStatus = 'idle';
            }, 2500);
        } catch (e) {
            error = String(e);
            saveStatus = 'error';
        }
    }

    const thresholdDirty = $derived(
        Math.max(1, Math.round(savedThresholdBytes / MIB)) !== thresholdMib,
    );

    onMount(() => {
        load();
        const unlisten: Promise<UnlistenFn> = onDaemonReconnected(load);
        return () => {
            unlisten.then((u) => u()).catch(() => {});
        };
    });
</script>

<main>
    <header>
        <h1>Settings</h1>
    </header>

    {#if loading && apps.length === 0}
        <p class="hint">Loading…</p>
    {:else if error}
        <div class="error">
            <p><strong>Couldn't reach the daemon.</strong></p>
            <p class="detail">{error}</p>
            <p class="hint">Is <code>ludex-daemon</code> running?</p>
        </div>
    {:else}
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
                    disabled={!thresholdDirty || saveStatus === 'saving'}
                >
                    {#if saveStatus === 'saving'}Saving…{:else}Save threshold{/if}
                </button>
                {#if saveStatus === 'saved'}
                    <span class="hint">Saved — takes effect on next daemon restart.</span>
                {:else if thresholdDirty}
                    <span class="hint"
                        >Unsaved change — takes effect after daemon restart.</span
                    >
                {/if}
            </div>
        </section>

        <section>
            <h2>Blocked games</h2>
            <p class="description">
                Blocked games stop recording new sessions and are hidden from
                the Games and Recent views. Their history stays in the database
                — unblock here to see them again.
            </p>
            {#if apps.length === 0}
                <p class="hint">No applications tracked yet.</p>
            {:else}
                <ul class="apps">
                    {#each apps as app (app.id)}
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

    .description {
        color: var(--text-muted);
        font-size: 0.88rem;
        margin: 0 0 1rem;
        line-height: 1.5;
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

    input[type='number'] {
        font: inherit;
        padding: 0.45rem 0.6rem;
        border: 1px solid var(--button-border);
        background: var(--bg-surface);
        color: var(--text-primary);
        border-radius: 6px;
        font-variant-numeric: tabular-nums;
    }

    input[type='number']:focus {
        outline: 2px solid var(--accent);
        outline-offset: -1px;
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
</style>
