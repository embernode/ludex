<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        getAltTabGraceSeconds,
        getIdleGraceSeconds,
        onDaemonReconnected,
        setAltTabGraceSeconds,
        setIdleGraceSeconds,
    } from '$lib/api';
    import SettingsCard from './SettingsCard.svelte';
    import NumberSetting from './NumberSetting.svelte';
    // Alt-tab grace is meaningless when focus loss never pauses, so
    // the field follows the Detection card's toggle. Shared rather
    // than re-read here: a private copy would stay stale for the rest
    // of the visit once the user flipped that switch.
    import { pausesOnFocusLoss } from './detectionState.svelte';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    let altTabSeconds = $state<number>(15);
    let cutsceneSeconds = $state<number>(300);

    async function load() {
        try {
            const [grace, idle] = await Promise.all([
                getAltTabGraceSeconds(),
                getIdleGraceSeconds(),
            ]);
            altTabSeconds = Math.max(0, Math.round(grace));
            cutsceneSeconds = Math.max(0, Math.round(idle));
        } catch (e) {
            onerror?.(String(e));
        }
    }

    async function commitAltTab(seconds: number) {
        await setAltTabGraceSeconds(seconds);
        altTabSeconds = seconds;
    }

    async function commitCutscene(seconds: number) {
        await setIdleGraceSeconds(seconds);
        cutsceneSeconds = seconds;
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

<SettingsCard
    title="Grace windows"
    subtitle="How long ludex keeps counting after focus or input stops."
>
    <NumberSetting
        label="Alt-tab grace"
        help="Brief focus loss doesn't end the session. 0 ends it immediately."
        unit="s"
        bounds={{ min: 0, max: 600 }}
        value={altTabSeconds}
        disabled={!pausesOnFocusLoss()}
        commit={commitAltTab}
    />

    <NumberSetting
        label="Cutscene grace"
        help="Input silence forgiven before the interactive clock pauses. Only the first N seconds of each idle interval are credited; the rest is still subtracted."
        unit="s"
        bounds={{ min: 0, max: 3600 }}
        value={cutsceneSeconds}
        commit={commitCutscene}
    />
</SettingsCard>
