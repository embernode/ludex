<script lang="ts">
    // Home page. Wired to the daemon's net.ludex.Tracker1 D-Bus
    // service in milestone M6.3; this scaffold is a placeholder that
    // confirms the Tauri + SvelteKit + Vite toolchain is alive.
</script>

<main>
    <header>
        <h1>ludex</h1>
        <span class="tag">pre-alpha</span>
    </header>

    <section>
        <p>
            Playtime tracker for Linux. The daemon runs in the background; this
            window is the desktop UI.
        </p>
        <p>
            The data pipe from <code>net.ludex.Tracker1</code> lands in the next
            milestone — this screen will show your applications, recent sessions,
            and playtime dashboards once wired up.
        </p>
    </section>
</main>

<style>
    main {
        max-width: 64ch;
        margin: 0 auto;
        padding: 3rem 2rem;
    }

    header {
        display: flex;
        align-items: baseline;
        gap: 0.75rem;
        margin-bottom: 1.5rem;
    }

    h1 {
        font-size: 2.5rem;
        font-weight: 600;
        margin: 0;
        letter-spacing: -0.02em;
    }

    .tag {
        font-size: 0.8rem;
        padding: 0.15rem 0.5rem;
        border-radius: 999px;
        background: #eee;
        color: #666;
        font-weight: 500;
    }

    section p {
        line-height: 1.55;
        color: #444;
    }

    code {
        font-family: 'JetBrains Mono', 'Fira Mono', ui-monospace, monospace;
        font-size: 0.9em;
        background: #eceef2;
        padding: 0.1em 0.35em;
        border-radius: 4px;
    }
</style>
