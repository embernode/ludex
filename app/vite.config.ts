import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Tauri expects a fixed port; fail fast if unavailable.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
    plugins: [sveltekit()],
    clearScreen: false,
    build: {
        // The dashboard route bundles ECharts (~575 kB pre-gzip) which
        // is irreducible at this feature set — Bar/Line/Heatmap chart
        // types plus Calendar/Grid/Tooltip components are all in use.
        // Vite's 500 kB default is calibrated for web apps where users
        // wait on downloads; a Tauri WebView loads from disk so the
        // warning is cosmetic here. Raise just enough to clear it.
        chunkSizeWarningLimit: 700,
    },
    server: {
        port: 1420,
        strictPort: true,
        host: host || false,
        hmr: host
            ? { protocol: 'ws', host, port: 1421 }
            : undefined,
        watch: {
            // Don't re-trigger the dev server on Rust edits.
            ignored: ['**/src-tauri/**'],
        },
    },
});
