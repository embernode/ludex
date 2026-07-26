# ludex — architecture

## Goals

- Launcher-agnostic automatic detection of games being played on Linux.
- No per-game configuration required for games launched through mainstream launchers (Steam/Proton, Lutris, Heroic).
- No telemetry; no network I/O at runtime.
- Accurate split between *full* session runtime and *interactive* runtime (wall time minus idle intervals).
- Correct, recoverable session accounting across daemon crashes, sleep/wake, and reboots.
- Publishable at any point — code quality, structured logging, and tests held to that bar.

## Non-goals (v1)

- Flatpak packaging.
- First-class Steam Deck gaming-mode support (gamescope is handled when encountered, but not a target surface).
- Global input monitoring via `/dev/input/event*` (opt-in behind a feature flag).
- Save-file backup.
- Overlays, in-game notifications, text-to-speech.
- Windows support.

## Process model

```
┌─────────────────────────────────────────────────────────────────────┐
│ ludex-daemon (systemd --user)                                       │
│                                                                     │
│  ┌─────────────┐    ┌──────────────┐   ┌────────────────────────┐   │
│  │   Sources   │──▶│   Detector   │──▶│    SessionManager      │   │
│  │             │    │              │   │                        │   │
│  │ Steam log   │    │ blocklist /  │   │ SQLite (WAL)           │   │
│  │ Lutris dbus │    │ forcelist    │   │ heartbeat every 60 s   │   │
│  │ Heroic file │    │ is-fullscreen│   │ cold-start recovery    │   │
│  │ KWin focus  │    │ GPU fdinfo   │   │ pidfd exit wait        │   │
│  │ /proc poll  │    │              │   │ idle subtract          │   │
│  └─────────────┘    └──────────────┘   └────────────────────────┘   │
│          │                                        │                 │
│          └──────────────▶ mpsc channels ◀─────────┘                 │
│                                                                     │
│             D-Bus service: net.ludex.Tracker1                       │
└───────────────────────────────┬─────────────────────────────────────┘
                                │
              ┌─────────────────┴──────────────────┐
              ▼                                    ▼
       ┌────────────┐                       ┌────────────┐
       │  ludex-gui │                       │  ludex-cli │
       │  (Tauri)   │                       │            │
       └────────────┘                       └────────────┘
```

The daemon is split into three actor-style layers that communicate over `tokio::sync::mpsc`. Every edge has a defined message type; no shared mutable state crosses a boundary. This keeps each layer independently testable and keeps the main loop trivially recoverable after a bug in one source.

- **Sources** emit `GameEvent::Started { key, at }` / `Stopped { key, at }`. `GameKey` is `(launcher_type, launcher_id)` for launcher-attributed games, or `("native", canonical_exe_path)` for the fallback path.
- **Detector** consumes events, applies the blocklist / forcelist / heuristic gate, and emits `DetectedGame { application_id }` on acceptance.
- **SessionManager** owns the SQLite handle, opens/closes sessions, manages heartbeats, subtracts idle time, and surfaces D-Bus signals.

## Detection flow

### Primary: launcher-state subscriptions

| Launcher | Mechanism | Event source | Status |
|---|---|---|---|
| Steam | inotify | `~/.local/share/Steam/logs/content_log.txt` — parses `AppState changed : <appid> : Running\|Stopped` lines; cross-references `appmanifest_<appid>.acf` for the canonical name. Rotated logs handled. | Shipped |
| Lutris | *Foreground + enrich* | No native start/stop signal — `net.lutris.Lutris` exists on the session bus but doesn't emit game lifecycle events. The Wayland-foreground source picks up Lutris-launched processes (`LUTRIS_GAME_UUID` is intentionally absent from the gate's launcher-attribution rejection set), and the `pga.db` enricher fills in product names. Battle.net's catalogue is curated by executable basename. | Shipped |
| Heroic | *Foreground + env-var attribution + enrich* | No native start/stop signal — Heroic 2.x removed `~/.config/heroic/running_game.json` and exposes nothing equivalent. The Wayland-foreground source accepts the game process; `HEROIC_APP_NAME` from the process environ is used as the launcher_id (canonical and wine-variant-invariant — Heroic lets users pick a wine/Proton variant per game). The enricher reads the runner-specific library caches under `~/.config/heroic/store_cache/` (`legendary_library.json`, `gog_library.json`, `nile_library.json`) and fills in title, developer, and the real Windows .exe path. | Shipped |

Steam has no public D-Bus API for game start/stop events; filesystem watching is the stable approach. Rotating backups to a process-tree scan (`reaper`, `steam-launch-wrapper` descendants) is a documented fallback if the log format changes.

For Heroic specifically, processes running under Proton inherit `STEAM_COMPAT_APP_ID`; the gate has a second env-var category (`HEROIC_APP_NAME`, `LUTRIS_GAME_UUID`) whose presence overrides the Steam-attribution rejection so Heroic-via-Proton and Lutris-via-Proton launches don't get silently dropped.

**Cold-start ordering.** On daemon start the sources subscribe first, then perform an enumeration of already-running games. Events fired during the enumeration are queued in the subscription, not lost.

### Fallback: Wayland foreground + GPU gate

For games launched outside any recognized launcher (indie bundles, CLI-launched emulators, self-compiled builds), detection is:

1. **Foreground-window source**: a KWin script registered by the daemon forwards `workspace.windowActivated` signals over D-Bus. X11 fallback reads `_NET_ACTIVE_WINDOW`.
2. **Process identification**: window → PID (from the compositor) → `/proc/<pid>/exe` (canonical path) → `/proc/<pid>/maps` (loaded library list — match `libGL.so*`, `libvulkan.so*`, `libEGL.so*`, `libSDL2*.so*`, and for Proton/Wine: `dxvk*.dll`, `vkd3d*.dll`, `wined3d*.dll`).
3. **Decision gate** (runs only for unknown PIDs, not for launcher-attributed games which are trusted):
   - **Reject** if: the window is not fullscreen relative to its output; PID's exe path is under `/usr`, `/bin`, `/sbin`, the compositor binary, or the screen-saver / lock-screen binary; PID is the desktop shell itself.
   - **Accept** if: a graphics library is loaded **AND** (the window is fullscreen **OR** per-process GPU usage crosses a configurable threshold).
4. **GPU usage** is read from `/proc/<pid>/fdinfo/*` (the kernel's DRM fdinfo standard): `drm-engine-<name>` for GPU time, `drm-memory-<name>` for VRAM. Sampled at a ≥ 2 s cadence, only for the single candidate foreground PID — never a system-wide scan.
5. **Gamescope handling**: the gate trusts a candidate whose *ancestry* includes `gamescope`/`gamescope-wl`, bypassing the fullscreen / graphics-library checks. Under KWin the foreground PID is usually gamescope itself (it owns the surface KWin reports), so this ancestry bypass seldom triggers there; gamescope-wrapped games are instead accepted via the fullscreen path, and launcher-attributed (Heroic/Lutris) titles name correctly through their inherited id env var. A native game launched under gamescope is recorded under the `gamescope` binary — a known, accepted limitation. Graceful degradation rather than a first-class feature.

The "AND graphics-library-loaded" requirement is tighter than it would be on Windows. On Linux, "loaded libGL" is a strong signal because ordinary desktop applications use Qt/GTK, which don't load raw GL. On Windows the equivalent DLL check is weakened by Electron and every browser pulling in `dxgi.dll`.

### Multi-monitor correctness

Fullscreen is tested against the output that contains the window, not the primary output. A game fullscreened on a secondary monitor is still a game.

### Emulator ROM tracking

For a process whose exe matches a configured emulator, `readlink /proc/<pid>/fd/*` yields all open files. Files whose paths match the emulator's configured ROM glob patterns (for example `*.iso`, `*.rpx`, `*.gcm`) identify the currently-loaded ROM. Changing ROMs without exiting the emulator ends the current session and starts a new one keyed to the new ROM.

## Metadata enrichment

Detection answers "is this a game?". Identification answers "what do we call it?". These are deliberately separate so that an identification miss doesn't break session tracking.

For a newly accepted game, identification runs a cascade of sources in ascending priority (each can overwrite the previous):

1. Exe basename (last-resort fallback).
2. `.desktop` file match for the exe path or launcher id (searched under `~/.local/share/applications/`, `/usr/share/applications/`, `~/.local/share/flatpak/exports/share/applications/`).
3. PE `FileVersionInfo` for Proton/Wine games, parsed from the on-disk exe via the `pelite` crate. No Wine runtime needed.
4. GOG `goggame-*.info` JSON (found by walking up the exe's directory — common in Heroic-managed installs).
5. Heroic JSON canonical name.
6. Lutris DB canonical name (from its SQLite metadata).
7. Steam `appmanifest_<appid>.acf` canonical name (VDF-ish key-value).

Each parser is **property-tested** against committed fixture files. Path-parsing bugs are a classic latent-crash source in this category of tool; the parsers are kept dumb, total, and exhaustively tested.

## Identity

Primary key is `(launcher_type, launcher_id)`:

- Steam → `("steam", "<appid>")`
- Lutris → `("lutris", "<slug>")`
- Heroic → `("heroic", "<app_name>")`
- Flatpak native → `("flatpak", "<app-id>")`
- Everything else → `("native", <canonical_exe_path>)`

Install paths change across updates; launcher IDs don't. The canonical exe path is used only for the native fallback bucket.

Even when a game is detected via the fallback path, if the enrichment cascade resolves a launcher id (for example by matching the exe to a Steam library entry), identity is rewritten to the launcher form. This keeps the database joinable against external catalogues (SteamGridDB, HowLongToBeat, ProtonDB) by stable identifier rather than by name.

## Storage

SQLite, WAL mode, `PRAGMA foreign_keys=ON`, busy-timeout configured. Migrations managed by `sqlx migrate` from commit 1 — no schema evolution in place.

Table sketch (full DDL in `crates/ludex-core/migrations/`):

- **`applications`** — one row per tracked game. Columns: `id`, `launcher_type`, `launcher_id`, `product_name`, `publisher`, `version`, `executable_path`, `launcher_exe_path`, `wineprefix_path`, `installed_flatpak_ref`, `graphics_platform`, `process_architecture`, `group_id`, `detected_via` (which enrichment source supplied `product_name`; nullable, and deliberately carries no CHECK so adding an enricher needs no table rebuild), icon BLOBs, aggregate statistics columns.
- **`sessions`** — one row per play session. `application_id`, `started_at`, `ended_at` (nullable until close), `heartbeat_at`, `full_runtime_seconds`, `interactive_runtime_seconds`, `exit_reason` (`terminated | foreground_changed | recovered`; `sleep_split` is reserved in the CHECK constraint but not yet produced).
- **`blocked_applications`** / **`forced_applications`** — user-maintained exe/launcher-id lists for the fallback path. The forced list is schema-ready but has no gate-layer consumer yet (see the GUI backlog in `roadmap.md`).
- **`emulators`** + **`emulator_platforms`** + **`emulator_platform_filename_patterns`** — emulator ROM-tracking configuration. Schema-ready; the ROM-tracking consumer hasn't shipped.
- **`groups`** — genre buckets. Seeded, but nothing assigns a group yet (see the genre-donut entry in `roadmap.md`).
- **`schema_info`** — key/value for migration version etc.

There is no per-day rollup table: daily aggregates are computed live from `sessions` (a `statistics_daily` table from schema v1 never gained a writer and was dropped in migration 0004). Aggregation buckets by the daemon's *local* calendar day via SQLite's `localtime` modifier — timestamps stay UTC in the database, only the grouping converts, with DST resolved per timestamp. The daemon and its clients share a session bus and therefore a timezone, so no offset needs to travel over D-Bus. A session's whole runtime lands on its start day.

## Session lifecycle

- **Start**: `SessionManager.begin(application_id, started_at)` opens a row with `ended_at = NULL`.
- **Heartbeat**: every 60 seconds the manager writes the current wall-clock time into `heartbeat_at` and flushes WAL. A daemon crash loses at most one minute of runtime.
- **Idle subtraction**: the idle source subscribes to `org.freedesktop.login1.Session.PropertiesChanged` for `IdleHint`. When idle-in, runtime accumulation continues on `full_runtime_seconds` but pauses for `interactive_runtime_seconds`.
- **Process-exit**: `pidfd_open(pid, 0)` + `poll()` on `POLLIN`. Kernel-level, zero-polling wait per-process.
- **End**: on any of process-exit, launcher stop-event, or explicit foreground change, the session row is closed with `ended_at` and `exit_reason`.
- **Sleep/wake**: suspend is detected by wall-vs-monotonic clock drift (≥ 5 s of drift per tick reads as a suspend — more reliable than `PrepareForSleep`, whose pre-suspend half can fire after the daemon is already frozen; see `sleep.rs`). Suspended seconds are subtracted from both runtime figures rather than splitting the session; the `sleep_split` exit reason is reserved for a future boundary-split implementation.
- **Cold-start recovery**: on daemon start, any session row with `ended_at IS NULL` whose `heartbeat_at` is older than the grace-period is closed at its last heartbeat with `exit_reason = 'recovered'`. No "8,000-hour Skyrim run" after a crash.

## IPC

`net.ludex.Tracker1` on the session bus.

Timestamps cross the bus as RFC 3339 strings, and an empty string means
"none" — D-Bus has no null. Struct signatures are derived from the DTOs
in `ludex-dbus-types`, so they are not restated here; adding a field to
one of those structs changes the signature and both ends must be
rebuilt together.

Applications and sessions:
- `ListApplications() -> a(...)` — every tracked application.
- `GetApplication(id: x) -> a(...)` — 0-or-1 element, standing in for an
  optional return.
- `ListRecentSessions(limit: u) -> a(...)` — newest first, with adjacent
  same-application fragments folded into one row.
- `ListSessionsInRange(from: s, to: s) -> a(...)` — every session
  overlapping the half-open window, oldest first and *unfolded*. Bounded
  by the window rather than a row count, because the activity grid needs
  all of a day's sessions and a newest-N fetch drops the older ones
  without any sign that it did.
- `ListSessionsForApplication(...)`.
- `ListDailyPlaytime(days: u) -> a(...)` — one entry per day that has
  sessions, bucketed by local calendar day.
- `DeleteSession(ids: ax) -> b` — deletes an explicit set of ids and
  recomputes the affected applications' totals.

Blocklist:
- `ListBlockedApplicationIds() -> ax`.
- `BlockApplication(id: x)` / `UnblockApplication(id: x)`.

Settings, each a getter/setter pair applied live by the daemon:
`GpuMemoryThresholdBytes`, `AltTabGraceSeconds`, `IdleGraceSeconds`,
`PauseWhenBackgrounded`, `BackupIntervalHours`, `BackupRetentionCount`.

Backups:
- `TakeBackupNow() -> s` — path of the snapshot written.
- `GetBackupStats() -> (...)`.

Signals:
- `ApplicationAdded(application_id: x)`.
- `SessionStarted(application_id: x)`.
- `SessionEnded(application_id: x, full: x, interactive: x)`.

No methods require privileges beyond session-bus access.

## Logging and diagnostics

`tracing` with `tracing-subscriber`. Every detection decision emits a span (`source=steam appid=... decision=accept reason=launcher-event`) so a misdetection can be replayed from logs without re-running the scenario. Default output is pretty to the journal; JSON output is available via `LUDEX_LOG=json`.

A `ludex doctor` subcommand prints a capabilities snapshot: Steam directory present, Lutris D-Bus name owned, Heroic config dir present, KWin version, compositor (Wayland/X11), DRM fdinfo sample parsed correctly, logind reachable, `input` group membership (for the optional evdev path). This is the lowest-effort diagnostic every support ticket will start with.

## Risks and constraints

- **Steam has no official D-Bus API.** The content-log inotify approach is stable as of 2026 but is not a contract. A regression test fixture is committed; when the log format changes (rarely but it has happened), the parser is the affected unit and has fallback paths (`appmanifest_*.acf` mtime watching).
- **KWin scripting API** is the most ergonomic foreground-window source on Wayland today; it requires installing a script at daemon start. The fallback is a `wayland-client` crate implementation of `org_kde_plasma_window_management_v2`.
- **NVIDIA DRM fdinfo** exposes `drm-engine-*` from driver 550+. Per-process VRAM reporting stabilized in more recent releases. The GPU-usage check degrades gracefully when VRAM stats are missing (uses GPU time only).
- **Tauri + WebKitGTK in sandboxed runtimes** is a known source of runtime-version-skew bugs. Native packaging (the in-repo Arch `PKGBUILD`, distributed via GitHub Releases) is the primary distribution path; sandboxed bundle formats are not roadmapped.
- **Wayland prohibits global input monitoring.** Idle detection uses `logind.IdleHint`, which works without special permissions. Per-input-event counting via `/dev/input/event*` is feature-flagged off and requires `input` group membership; it is not enabled by default.
