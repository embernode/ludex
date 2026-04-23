# ludex

A launcher-agnostic playtime tracker for Linux.

**Status: pre-alpha.** The daemon, CLI, and Tauri GUI all build and run end-to-end; Steam-launched sessions are detected and recorded. The detection set is still being filled in — Lutris and Heroic launcher integrations are deferred (see the roadmap for the current tranche status).

## What it does

ludex records time spent playing games on Linux without requiring per-game configuration. The daemon observes game launches from:

- **Steam** via inotify on the Steam content log *(shipped)*
- **Lutris** via its `net.lutris.Lutris` D-Bus service *(deferred — Lutris does not expose start/stop signals on its session-bus interface today)*
- **Heroic Games Launcher** via process-tree / log inspection *(deferred — Heroic 2.x removed the single `running_game.json` file this was designed around)*
- **Anything else** via a Wayland foreground-window fallback gated on loaded graphics libraries and DRM fdinfo GPU-usage metrics *(shipped on KDE Plasma 6 Wayland)*

Each recognised game has its sessions persisted to SQLite with two runtime figures: **full runtime** (wall-clock session duration) and **interactive runtime** (full runtime minus system-reported idle intervals via `logind.IdleHint`).

The primary target is KDE Plasma 6 on Wayland; X11 is supported where the mechanisms collapse to `_NET_ACTIVE_WINDOW` and friends.

## Design principles

- **No telemetry.** No network I/O at runtime, full stop.
- **Minimal required permissions.** Unprivileged user account, session D-Bus only, no `input`/`video` group, no `sudo`.
- **Event-driven by preference.** Filesystem watches, D-Bus signals, `pidfd` waits. Polling is reserved for GPU-usage sampling of a single foreground process.
- **Separation of detection from identification.** The detector answers "is this a game?" with a small, fast gate; identification is a separate metadata cascade that can be wrong without breaking session tracking.
- **Structured logs, migration-backed storage, property-tested parsers** from the first commit.

## Architecture

- Rust daemon (`ludex-daemon`) runs as a systemd user service. Tokio for async, zbus for D-Bus, sqlx for SQLite, tracing for structured logs.
- Tauri + Svelte + ECharts GUI (`ludex-gui`) for dashboards and configuration (post-M5 milestone).
- D-Bus IPC (`net.ludex.Tracker1`) between daemon and GUI.

Full architecture in [docs/architecture.md](docs/architecture.md). Phased plan in [docs/roadmap.md](docs/roadmap.md).

## Repository layout

```
Cargo.toml            # Rust workspace manifest
crates/
  ludex-core/         # shared types, schema, SQL, errors
  ludex-daemon/       # the tracker (binary)
  ludex-cli/          # CLI client (binary)
  ludex-enrich/       # metadata enrichment cascade (desktop/Steam/GOG/PE)
  ludex-dbus-types/   # wire types shared by daemon and GUI
app/
  src/                # SvelteKit frontend (TypeScript + Svelte 5)
  src-tauri/          # Tauri 2 host binary
packaging/            # systemd service, PKGBUILD
docs/                 # architecture, roadmap
```

## Building

Workspace layout:

- `crates/ludex-daemon` and `crates/ludex-cli` are pure Rust and build
  with `cargo build`.
- `app/src-tauri` is the Tauri host for the desktop UI. Building or
  running it needs WebKitGTK 4.1 + javascriptcoregtk 4.1 installed at
  system level (Arch: `pacman -S webkit2gtk-4.1`).

### Running the UI in dev mode

```sh
cd app
pnpm install          # one-off; lockfile is committed
pnpm tauri dev        # launches the webview + hot-reloads Svelte edits
```

See [`docs/roadmap.md`](docs/roadmap.md) for the phased milestone
plan.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. Both require attribution via preservation of the copyright notice. Rust-community convention.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
