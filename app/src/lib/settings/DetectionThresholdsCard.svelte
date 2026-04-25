<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        getGpuMemoryThresholdBytes,
        onDaemonReconnected,
        setGpuMemoryThresholdBytes,
    } from '$lib/api';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    /** MiB <-> bytes (we show mebibytes in the UI). */
    const MIB = 1024 * 1024;

    let savedBytes = $state<number>(0);
    let mib = $state<number>(50);
    let status = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    const dirty = $derived(Math.max(1, Math.round(savedBytes / MIB)) !== mib);

    async function load() {
        try {
            const bytes = await getGpuMemoryThresholdBytes();
            savedBytes = bytes;
            mib = Math.max(1, Math.round(bytes / MIB));
        } catch (e) {
            onerror?.(String(e));
        }
    }

    async function save() {
        const bytes = Math.max(1, Math.floor(mib * MIB));
        status = 'saving';
        try {
            await setGpuMemoryThresholdBytes(bytes);
            savedBytes = bytes;
            status = 'saved';
            setTimeout(() => {
                if (status === 'saved') status = 'idle';
            }, 2500);
        } catch (e) {
            onerror?.(String(e));
            status = 'error';
        }
    }

    onMount(() => {
        load();
        const unlistens: Promise<UnlistenFn>[] = [onDaemonReconnected(load)];
        return () => {
            for (const p of unlistens) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<section class="settings-card">
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
            bind:value={mib}
        />
    </label>
    <div class="actions">
        <button
            type="button"
            onclick={save}
            disabled={!dirty || status === 'saving'}
        >
            {#if status === 'saving'}Saving…{:else}Save threshold{/if}
        </button>
        {#if status === 'saved'}
            <span class="hint">Saved.</span>
        {:else if dirty}
            <span class="hint">Unsaved change.</span>
        {/if}
    </div>
</section>
