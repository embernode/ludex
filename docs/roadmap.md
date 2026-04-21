# ludex — roadmap

## Constraints

- **No telemetry.** Not even a version probe.
- **Minimal required permissions.** Unprivileged user account. Session D-Bus only. No `input` or `video` group membership. No root, `sudo`, or special capabilities. All writes under `$XDG_DATA_HOME` and `$XDG_CONFIG_HOME`.
- **Event-driven by preference.** Inotify for launcher logs and state files, D-Bus signals for Lutris and logind, KWin scripting for window focus, `pidfd` for process-exit. Periodic polling is reserved for GPU fdinfo sampling of a single foreground PID.
- **AUR-native packaging is the priority target.** Flatpak is not roadmapped; Steam Deck gaming-mode is not a tier-1 surface.
- **GitHub-publishable quality from the first commit.** No commented-out code. No TODO files committed. No personal data in the repo. No `unwrap()` in production paths. Structured logging over `eprintln!`. Public APIs are documented.

## Tech stack

| Layer | Choice |
|---|---|
| Daemon language | Rust (stable, edition 2021) pinned via `rust-toolchain.toml` |
| Async runtime | `tokio` |
| D-Bus | `zbus` (async; session bus only) |
| SQLite | `sqlx` with compile-time checked queries + offline metadata |
| Logging | `tracing` + `tracing-subscriber` |
| Errors | `thiserror` for library crates, `anyhow` at binary edges |
| Filesystem watching | `notify` (inotify backend) |
| Syscalls | `rustix` (`pidfd`, `/proc` helpers) |
| Wayland fallback | KWin D-Bus scripting API (primary); `wayland-client` crate (fallback) |
| PE parsing | `pelite` |
| CLI | `clap` v4 (derive) |
| GUI | Tauri 2 + SvelteKit + ECharts |

## Repository layout (target)

```
Cargo.toml                 # workspace manifest
rust-toolchain.toml
rustfmt.toml
clippy.toml
.editorconfig
.github/
  workflows/ci.yml         # fmt, clippy -Dwarnings, test, cargo-deny
crates/
  ludex-core/              # shared types, schema, SQL, errors
    migrations/
    src/
  ludex-daemon/            # the tracker (binary)
    src/
      sources/             # launcher watchers + Wayland fallback
      detector/            # gate / decision
      session/             # lifecycle + persistence
      dbus/                # Tracker1 service
  ludex-cli/               # CLI client (binary) — talks to daemon over D-Bus
  xtask/                   # cargo xtask helpers (packaging, lint, release)
app/                       # Tauri + SvelteKit frontend (post-M5)
  src/
  src-tauri/
packaging/
  systemd/ludex-daemon.service
  PKGBUILD                 # AUR
docs/                      # this file, architecture.md
```

## Milestones

Each milestone ends with: all crates compile, `cargo clippy --all-targets -- -D warnings` clean, `cargo fmt --check` clean, tests green, tagged `v0.x.0-mN`.

### M0 — Repository scaffold

- Workspace manifest, pinned toolchain.
- `rustfmt.toml` (project defaults; no customisation unless justified).
- `clippy.toml` with `warn(clippy::pedantic, clippy::nursery)` and a curated allow-list for false-positives.
- `.editorconfig`.
- `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all`, `cargo deny check` (license/advisory audit).
- `ludex doctor` subcommand printing a capabilities table: Steam dir present / Lutris D-Bus name owned / Heroic config dir present / KWin version / Wayland or X11 / DRM fdinfo sample / logind reachable / `input` group membership.

This milestone is intentionally a no-op for end users. Its purpose is to prove the toolchain, the CI lane, and the environment detection layer before any detection code is written.

### M1 — Core schema and storage

- `sqlx` migrations: `applications`, `sessions`, `statistics_daily`, `blocked_applications`, `forced_applications`, `emulators` (+ platforms and patterns), `groups`, `schema_info`.
- Domain types in `ludex-core` with `sqlx::FromRow` impls.
- WAL mode, `PRAGMA foreign_keys=ON`, busy-timeout.
- Round-trip property tests for every persisted type.
- `ludex-core` exposes an `ApplicationRepo` / `SessionRepo` surface; no SQL leaks into `ludex-daemon`.

### M2 — Launcher sources (the MVP)

The user-visible value of this milestone: launch a Steam game, stop it, and see a session row in the DB.

Delivered in tranches because the three launchers are in different cold reality:

**M2.1 — Steam + session manager + daemon wiring (primary MVP)**

- [`GameEvent`] enum passed on a `tokio::sync::mpsc` channel from sources to the [`SessionManager`].
- [`SteamSource`]: filesystem watcher on `~/.local/share/Steam/logs/content_log.txt` parsing `state changed : ..., App Running, ...` transitions; appmanifest `name` correlation; cold-start scan of `appmanifest_*.acf` `StateFlags` bit 64 so already-running games are picked up at daemon start.
- [`SessionManager`]: opens sessions on `Started`, closes on `Stopped`, heartbeats every 60 s, closes dangling open sessions both at graceful shutdown (with `Terminated`) and at cold start (with `Recovered`, via `recover_orphans`).

**M2.2 — Heroic + Lutris (deferred until their detection shape is settled)**

- Modern Heroic (2.x) removed the single `running_game.json` file; the remaining options are log-tailing or process-tree inspection. Deferred until we have a stable story.
- Lutris exposes `net.lutris.Lutris` on the session bus but does not emit game start/stop signals there. Detection requires either `lutris-wrapper` process-tree scanning (belongs in M4) or a Lutris upstream change.

**M2.3 — CLI query surface**

- `ludex sessions [--since T]` lists recent sessions. Reads the database directly; no D-Bus service is required until the GUI lands in M6.

**Cold-start ordering** is preserved across tranches: the daemon installs every live subscription *before* it performs any enumeration of already-running games, so transitions that occur during the enumeration are queued in the subscription, not lost.

### M3 — Metadata enrichment cascade

- Steam `appmanifest_*.acf` parser (VDF-ish); property-tested against committed fixtures.
- Lutris `pga.db` SQLite reader.
- Heroic JSON reader.
- `.desktop` scanner.
- `pelite`-based PE `FileVersionInfo` extraction for Proton/Wine exes.
- GOG `goggame-*.info` JSON parser.
- Cascade runs on first-seen or on-demand re-enrichment; stores canonical name, publisher, version, icon bytes.

### M4 — Non-launcher detection fallback

- KWin scripting source: registers a script at daemon start that forwards `workspace.windowActivated` events over D-Bus.
- X11 fallback via `x11rb` for `_NET_ACTIVE_WINDOW` + `_NET_WM_PID`.
- `/proc/<pid>/maps` loaded-library probe for DirectX / OpenGL / Vulkan / SDL / Proton-translation DLLs.
- `/proc/<pid>/fdinfo/*` parser for DRM engine time and memory.
- Decision gate per architecture.md.
- Gamescope ancestry detection.
- Emulator ROM-file identification via `/proc/<pid>/fd/*` glob-matching.

### M5 — Idle detection and session lifecycle hardening

- `logind.IdleHint` watcher over D-Bus; maintains the `interactive_runtime_seconds` accounting.
- `PrepareForSleep` subscription; pauses / splits sessions across sleep/wake.
- `pidfd_open` + `poll` for process-exit.
- Cold-start recovery: close orphaned sessions at their last heartbeat.
- Optional feature flag `evdev` (off by default) for per-input-event counts when the user is in the `input` group; documented with the rationale and threat model.

### M6 — GUI

- Tauri 2 + SvelteKit app (`app/`).
- D-Bus client (`zbus` via Tauri command bridge).
- ECharts dashboards: per-game daily playtime line, calendar-heatmap, genre donut, sessions-this-week bar.
- Per-game detail view with all session rows and interactive/full split.
- Settings: idle threshold, GPU usage thresholds, block/force lists editor.
- System tray with current-session badge.

### Post-M6 (unscheduled)

- Save-file backup scoped to Proton prefixes, opt-in per game.
- Overlay or transient notifications.
- Localisation via `gettext`.
- `ludex migrate` — optional importer for users with per-game time data in other formats.

## Commit discipline

- **Style**: `<area>: <imperative subject, under ~70 chars>` → blank line → body explaining *why* when non-trivial. Valid areas: `core`, `daemon`, `cli`, `gui`, `docs`, `ci`, `pkg`, `repo`.
- **Atomic**. A commit touching two unrelated concerns is split.
- **No force-push to `main`.** Feature work on topic branches; squash-on-merge when the branch history is noisy.
- **Tags**: `vMAJOR.MINOR.PATCH`. Pre-M6 releases are `0.x`.
- **No attribution trailers** on any commit.
- **No personal data in commits**: no real email addresses in committed files, no real filesystem paths from the developer's machine, no screenshots that embed personal state.

## Development practices

- **Structured logging from commit 1.** `tracing` spans carry every detection input and decision.
- **Migrations from commit 1.** No schema evolution done in-place.
- **Property tests on every path parser.** Fuzz corpora committed under `crates/<...>/tests/fixtures/`.
- **Clippy `-D warnings`** in CI. Allow-list entries carry a comment explaining the false-positive.
- **`cargo deny`** in CI for license compatibility and security advisories.
- **MSRV pinned via `rust-toolchain.toml`.** Bumped with intent in a dedicated commit; not chased release-to-release.
- **Integration tests for the daemon** run against a temp `XDG_DATA_HOME` with fixture launcher state.
- **`ludex doctor` output is golden-file tested** against a synthetic environment.

## Open decisions

- **Minimum supported Plasma version.** Targeting 6.x only keeps the KWin scripting surface simple. 5.27 LTS support adds complexity; deferred unless a user requests it.
- **MSRV.** Pin to the current stable at M0 start; bump only when a dependency forces it.
