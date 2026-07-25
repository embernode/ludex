<script lang="ts">
    import { onMount } from 'svelte';
    import { goto } from '$app/navigation';

    // /dashboard and /recent merged into /activity, which shows the
    // same data as two panes of one view. Kept as a redirect so links
    // and any saved window state pointing here don't dead-end.
    onMount(() => {
        goto('/activity', { replaceState: true });
    });
</script>

<p class="hint">Moved to <a href="/activity">Activity</a>.</p>

<style>
    p {
        max-width: 1000px;
        margin: 0 auto;
        padding: 22px 20px;
    }
</style>
