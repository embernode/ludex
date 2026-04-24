# ludex

A launcher-agnostic playtime tracker for Linux.

**Status: pre-alpha.** Daemon, CLI, and Tauri GUI all build and run end-to-end. Steam-launched sessions are detected and recorded; the Wayland foreground-window fallback catches games launched outside any recognised launcher on KDE Plasma 6. The GUI covers the apps list, recent sessions, per-application detail, an ECharts dashboard, settings, and a system tray with close-to-tray. Lutris and Heroic launcher integrations remain deferred; no released binaries yet.

## What it does

ludex records time spent playing games on Linux without requiring per-game configuration. The daemon observes game launches from:

- **Steam** via inotify on the Steam content log *(shipped)*
- **Lutris** via its `net.lutris.Lutris` D-Bus service *(deferred — Lutris does not expose start/stop signals on its session-bus interface today)*
- **Heroic Games Launcher** via process-tree / log inspection *(deferred — Heroic 2.x removed the single `running_game.json` file this was designed around)*
- **Anything else** via a Wayland foreground-window fallback gated on loaded graphics libraries and DRM fdinfo GPU-usage metrics *(shipped on KDE Plasma 6 Wayland)*

Each recognised game has its sessions persisted to SQLite with two runtime figures: **full runtime** (wall-clock session duration) and **interactive runtime** (full runtime minus system-reported idle intervals via `logind.IdleHint`).

The primary target is KDE Plasma 6 on Wayland. X11 support is on the roadmap (the gate code already expects an `_NET_ACTIVE_WINDOW` source) but no X11 foreground source is wired up today — on X11 the launcher-based paths (Steam log tailing) still work, but the foreground fallback does not.

## Design principles

- **No telemetry.** No network I/O at runtime, full stop.
- **Minimal required permissions.** Unprivileged user account, session D-Bus only, no `input`/`video` group, no `sudo`.
- **Event-driven by preference.** Filesystem watches, D-Bus signals, `pidfd` waits. Polling is reserved for GPU-usage sampling of a single foreground process.
- **Separation of detection from identification.** The detector answers "is this a game?" with a small, fast gate; identification is a separate metadata cascade that can be wrong without breaking session tracking.
- **Structured logs, migration-backed storage, property-tested parsers** from the first commit.

## Architecture

- Rust daemon (`ludex-daemon`): tokio for async, zbus for D-Bus, sqlx for SQLite, tracing for structured logs. Designed to run as a `systemctl --user` service.
- Tauri 2 + SvelteKit + ECharts GUI (`ludex-gui`): apps list, recent sessions, per-app detail, dashboard, settings, system tray. Dark-mode toggle with OLED-friendly palette.
- CLI (`ludex`): operator tool — `doctor`, `apps list`, `sessions`, `backup {now,list,prune,restore}`, `merge`.
- D-Bus IPC over `net.ludex.Tracker1` on the user session bus. Wire types in the dedicated `ludex-dbus-types` crate.

Full design in [docs/architecture.md](docs/architecture.md); phased
plan + current backlog in [docs/roadmap.md](docs/roadmap.md); CLI
reference in [docs/cli.md](docs/cli.md).

## Repository layout

```
Cargo.toml            # Rust workspace manifest
rust-toolchain.toml   # pinned toolchain version
crates/
  ludex-core/         # shared types, schema, SQL, errors, backup engine
    migrations/       # SQLite schema migrations (sqlx::migrate!)
  ludex-daemon/       # the tracker (binary)
  ludex-cli/          # CLI client (binary)
  ludex-enrich/       # metadata enrichment cascade (desktop/Steam/GOG/PE)
  ludex-dbus-types/   # wire types shared by daemon and GUI
app/
  src/                # SvelteKit frontend (TypeScript + Svelte 5)
  src-tauri/          # Tauri 2 host binary
docs/                 # architecture, roadmap, CLI reference
.github/workflows/    # fmt, clippy, test, frontend, cargo-deny
```

A `packaging/` directory for the systemd user service and
PKGBUILD is planned (see the roadmap); there's nothing in it yet.

## Building

- `crates/ludex-daemon` and `crates/ludex-cli` are pure Rust and build
  with `cargo build --workspace`.
- `app/src-tauri` is the Tauri host for the desktop UI. Building or
  running it needs WebKitGTK 4.1 + javascriptcoregtk 4.1 installed at
  system level (Arch: `pacman -S webkit2gtk-4.1`). The tray icon
  also needs `libayatana-appindicator3`
  (`pacman -S libayatana-appindicator`).

Workspace-wide tests + lints:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Frontend type-check + build:

```sh
cd app
pnpm install          # one-off; lockfile is committed
pnpm run check        # svelte-check
pnpm run build        # static bundle for Tauri
```

### Running the daemon

For quick experimentation:

```sh
cargo run -p ludex-daemon            # runs in the foreground
```

For a long-running install, use the packaged systemd `--user`
unit — see [`packaging/README.md`](packaging/README.md):

```sh
cargo install --path crates/ludex-daemon
mkdir -p ~/.config/systemd/user
cp packaging/ludex-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now ludex-daemon
journalctl --user -u ludex-daemon -f
```

Data lands at `$XDG_DATA_HOME/ludex/ludex.sqlite` (or
`~/.local/share/ludex/ludex.sqlite` fallback). The daemon takes
periodic database snapshots at
`$XDG_DATA_HOME/ludex/backups/` by default.

Control logging via `LUDEX_LOG=info` (or `debug`, `trace`;
defaults to `info` for the daemon, `warn` for the CLI). Under
systemd, create a drop-in with `systemctl --user edit ludex-daemon`
to set `Environment="LUDEX_LOG=debug"`.

### Running the UI in dev mode

```sh
cd app
pnpm tauri dev        # launches the webview + hot-reloads Svelte edits
```

### Using the CLI

```sh
cargo install --path crates/ludex-cli
ludex doctor
```

Full subcommand reference (`apps` / `sessions` / `backup` /
`merge` / `doctor`) in [`docs/cli.md`](docs/cli.md).

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. Both require attribution via preservation of the copyright notice. Rust-community convention.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
