import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Tauri expects a fixed port; fail fast if unavailable.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
    plugins: [sveltekit()],
    clearScreen: false,
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
