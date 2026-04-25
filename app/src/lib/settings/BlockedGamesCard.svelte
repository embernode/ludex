<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        blockApplication,
        listApplications,
        listBlockedApplicationIds,
        onBlocklistChanged,
        onDaemonReconnected,
        unblockApplication,
        type ApplicationSummary,
    } from '$lib/api';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    let apps = $state<ApplicationSummary[]>([]);
    let blocked = $state<Set<number>>(new Set());

    /** Case-insensitive substring match against product name and
     *  publisher. Empty filter shows everything. */
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
        try {
            const [a, ids] = await Promise.all([
                listApplications(),
                listBlockedApplicationIds(),
            ]);
            apps = a;
            blocked = new Set(ids);
        } catch (e) {
            onerror?.(String(e));
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
        } catch (e) {
            onerror?.(String(e));
        }
    }

    onMount(() => {
        load();
        const unlistens: Promise<UnlistenFn>[] = [
            onDaemonReconnected(load),
            onBlocklistChanged(load),
        ];
        return () => {
            for (const p of unlistens) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<section class="settings-card blocked-section">
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

<style>
    /* Native <details> keeps a11y and keyboard support; we only
       restyle the summary so it reads like the other section
       headings. */
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

    .search {
        display: block;
        margin-bottom: 0.75rem;
    }

    .search input {
        width: 100%;
        box-sizing: border-box;
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
