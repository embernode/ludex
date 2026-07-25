<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        getGpuMemoryThresholdBytes,
        onDaemonReconnected,
        setGpuMemoryThresholdBytes,
    } from '$lib/api';
    import SettingsCard from './SettingsCard.svelte';
    import SettingRow from './SettingRow.svelte';
    import NumberSetting from './NumberSetting.svelte';
    import ToggleSwitch from './ToggleSwitch.svelte';
    import {
        loadPauseWhenBackgrounded,
        pausesOnFocusLoss,
        setPausesOnFocusLoss,
    } from './detectionState.svelte';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    /** MiB <-> bytes (we show mebibytes in the UI). */
    const MIB = 1024 * 1024;

    let thresholdMib = $state<number>(50);

    async function load() {
        try {
            const [bytes] = await Promise.all([
                getGpuMemoryThresholdBytes(),
                loadPauseWhenBackgrounded(),
            ]);
            thresholdMib = Math.max(1, Math.round(bytes / MIB));
        } catch (e) {
            onerror?.(String(e));
        }
    }

    async function commitThreshold(mib: number) {
        await setGpuMemoryThresholdBytes(Math.max(1, Math.floor(mib * MIB)));
        thresholdMib = mib;
    }

    async function togglePause(next: boolean) {
        try {
            await setPausesOnFocusLoss(next);
        } catch (e) {
            onerror?.(String(e));
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

<SettingsCard
    title="Detection"
    subtitle="Only affects the window fallback. Launcher-detected games ignore these."
>
    <NumberSetting
        label="GPU memory threshold"
        help="Lower catches windowed games with small VRAM footprints; higher keeps desktop apps out."
        unit="MiB"
        bounds={{ min: 1, max: 16384 }}
        value={thresholdMib}
        commit={commitThreshold}
    />

    <SettingRow
        label="Pause when the game loses focus"
        help="Off = sessions end only when the process exits."
    >
        {#snippet control()}
            <ToggleSwitch
                checked={pausesOnFocusLoss()}
                label="Pause when the game loses focus"
                onchange={togglePause}
            />
        {/snippet}
    </SettingRow>
</SettingsCard>
