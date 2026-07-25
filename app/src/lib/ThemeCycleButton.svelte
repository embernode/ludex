<script lang="ts">
    import { nextMode } from '$lib/theme';
    import { currentMode, cycleMode, MODE_LABEL } from '$lib/themeState.svelte';

    // A single button can't show three states the way a segmented
    // control can, so the accessible name points at where a click
    // goes rather than only where it is.
    const title = $derived(
        `Theme: ${MODE_LABEL[currentMode()]} — click for ${
            MODE_LABEL[nextMode(currentMode())]
        }`,
    );
</script>

<button
    type="button"
    class="theme-cycle"
    onclick={cycleMode}
    aria-label={title}
    {title}
>
    {MODE_LABEL[currentMode()]}
</button>

<style>
    /* Showing the mode as text rather than an icon is what makes three
       states workable in one control: an icon can't distinguish
       Auto-resolved-dark from Dark. */
    .theme-cycle {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        width: 74px;
        padding: 6px 0;
        border: 1px solid var(--line);
        border-radius: 7px;
        background: var(--surface);
        color: var(--fg2);
        font-size: 11.5px;
        font-weight: 500;
        cursor: pointer;
        flex: none;
    }

    .theme-cycle:hover {
        background: var(--surface);
        color: var(--fg);
    }
</style>
