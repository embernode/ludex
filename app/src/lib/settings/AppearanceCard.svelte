<script lang="ts">
    import { onMount } from 'svelte';
    import ThemeCycleButton from '$lib/ThemeCycleButton.svelte';
    import SettingsCard from './SettingsCard.svelte';
    import SettingRow from './SettingRow.svelte';
    import { ACCENTS, currentTheme, observeTheme, type Theme } from '$lib/theme';
    import {
        currentAccent,
        currentMode,
        MODE_NOTE,
        portalIsDriving,
        setAccent,
    } from '$lib/themeState.svelte';

    // What `auto` currently resolves to, for the help line. Tracked
    // through the attribute rather than recomputed so it follows a
    // desktop change while this page is open.
    let resolved = $state<Theme>(currentTheme());

    onMount(() => observeTheme((t) => (resolved = t)));

    const note = $derived.by(() => {
        const mode = currentMode();
        if (mode !== 'auto') return MODE_NOTE[mode];
        // Naming the mechanism matters here: without a portal this is
        // the webview's own guess, which on this desktop is often
        // wrong, and the user deserves to know which they are getting.
        return portalIsDriving()
            ? `Following the desktop colour scheme via the appearance portal — currently ${resolved}.`
            : `Following the browser's colour-scheme hint — currently ${resolved}. No appearance portal answered.`;
    });
</script>

<SettingsCard title="Appearance" subtitle="Colour scheme and accent.">
    <SettingRow label="Colour scheme" help={note}>
        {#snippet control()}
            <ThemeCycleButton />
        {/snippet}
    </SettingRow>

    <SettingRow
        label="Accent colour"
        help="Tints the active-session pill, bars, and charts. The tray icon keeps its fixed brand green."
    >
        {#snippet control()}
            <div class="swatches">
                {#each ACCENTS as { hex, name } (hex)}
                    <button
                        type="button"
                        class="swatch"
                        style="background:{hex}"
                        title={name}
                        aria-label="Accent: {name}"
                        aria-pressed={currentAccent() === hex}
                        onclick={() => setAccent(hex)}
                    ></button>
                {/each}
            </div>
        {/snippet}
    </SettingRow>
</SettingsCard>

<style>
    .swatches {
        display: flex;
        gap: 7px;
    }

    .swatch {
        width: 22px;
        height: 22px;
        border-radius: 6px;
        cursor: pointer;
        border: 2px solid transparent;
        padding: 0;
    }

    .swatch[aria-pressed='true'] {
        border-color: var(--fg);
    }

    .swatch:focus-visible {
        outline: 2px solid var(--fg);
        outline-offset: 2px;
    }
</style>
