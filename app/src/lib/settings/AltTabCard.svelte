<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        getAltTabGraceSeconds,
        getIdleGraceSeconds,
        getPauseWhenBackgrounded,
        onDaemonReconnected,
        setAltTabGraceSeconds,
        setIdleGraceSeconds,
        setPauseWhenBackgrounded,
    } from '$lib/api';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    let savedGraceSeconds = $state<number>(0);
    let graceSeconds = $state<number>(15);
    let graceStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    let pauseWhenBackgrounded = $state<boolean>(true);

    /** Per-idle-interval cutscene grace, exposed in MINUTES (the
     *  underlying setting is stored as seconds; minutes is the
     *  natural unit for the typical 5-minute default). */
    let savedIdleGraceMinutes = $state<number>(5);
    let idleGraceMinutes = $state<number>(5);
    let idleGraceStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

    const graceDirty = $derived(
        Math.max(0, Math.round(savedGraceSeconds)) !== graceSeconds,
    );
    const idleGraceDirty = $derived(
        Math.max(0, Math.round(savedIdleGraceMinutes)) !== idleGraceMinutes,
    );

    async function load() {
        try {
            const [grace, pause, idleSecs] = await Promise.all([
                getAltTabGraceSeconds(),
                getPauseWhenBackgrounded(),
                getIdleGraceSeconds(),
            ]);
            savedGraceSeconds = grace;
            graceSeconds = Math.max(0, Math.round(grace));
            pauseWhenBackgrounded = pause;
            // Round to whole minutes for the UI; sub-minute grace
            // is too short to matter.
            savedIdleGraceMinutes = Math.max(0, Math.round(idleSecs / 60));
            idleGraceMinutes = savedIdleGraceMinutes;
        } catch (e) {
            onerror?.(String(e));
        }
    }

    async function togglePauseWhenBackgrounded() {
        // The bind:checked above flipped the state already; persist
        // it, and revert the local flip on failure so the UI matches
        // reality.
        try {
            await setPauseWhenBackgrounded(pauseWhenBackgrounded);
        } catch (e) {
            pauseWhenBackgrounded = !pauseWhenBackgrounded;
            onerror?.(String(e));
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
            onerror?.(String(e));
            graceStatus = 'error';
        }
    }

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
            onerror?.(String(e));
            idleGraceStatus = 'error';
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
