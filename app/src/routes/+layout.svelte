<script lang="ts">
    // Root layout. All pages render inside here.
    let { children } = $props();
</script>

<div class="app">
    {@render children?.()}
</div>

<style>
    :global(html, body) {
        margin: 0;
        padding: 0;
        height: 100%;
        font-family: system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
        background: #f7f7f9;
        color: #1a1a1a;
    }

    .app {
        min-height: 100vh;
        display: flex;
        flex-direction: column;
    }
</style>
