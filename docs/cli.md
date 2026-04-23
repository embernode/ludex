# ludex — CLI reference

The `ludex` binary is the operator tool for a running `ludex-daemon`. It
reads and writes the same SQLite database at `$XDG_DATA_HOME/ludex/ludex.sqlite`
(falling back to `$HOME/.local/share/ludex/ludex.sqlite` when XDG_DATA_HOME
is unset). Most subcommands are safe to run while the daemon is up — the
exceptions are called out per-command.

## Installation

From the repository root:

```sh
cargo install --path crates/ludex-cli
```

drops `ludex` into `~/.cargo/bin`. Make sure that directory is on your
`$PATH`.

Without the install step every command below works as
`cargo run -p ludex-cli -- <subcommand>` from inside the repository.

## Logging

Every command honours the `LUDEX_LOG` environment variable, routed through
[`tracing_subscriber::EnvFilter`]. Default is `warn`. Useful values:

```sh
LUDEX_LOG=info  ludex …
LUDEX_LOG=debug,sqlx=warn  ludex …
```

Output goes to stderr; command stdout is reserved for the actual result
so you can pipe it into other tools.

## Subcommand reference

### `ludex doctor`

Prints a capability table for the current environment: XDG session type,
desktop, reachability of the KWin + logind D-Bus services, presence of the
Steam / Heroic data directories, DRM subsystem detection, `input` group
membership, and `pidfd` syscall support. Reads nothing from the daemon
and writes nothing — safe to run at any time.

Good for a quick "is this machine able to host ludex" check, and included
in any bug report.

### `ludex apps list`

Prints every tracked application with its numeric id, launcher key,
product name, run count, and last-played date. Newest-played first.

```
 id  launcher                                        application      runs  last played
─────────────────────────────────────────────────────────────────────────────────────────
  2  steam:1621690                                   Core Keeper        17  2026-04-18
  5  native:h:\pelit\steam\…\core keeper\corekee…    Core Keeper         1  2023-08-04
```

The id is what [`ludex merge`](#ludex-merge-src_id-dst_id) takes; the
GUI's detail page also shows it as a small `#N` chip next to the
publisher.

Reads the database directly — works even when the daemon is stopped.

### `ludex sessions [-n N]`

Prints the `N` most-recent sessions across every application, joined to
the owning application's product name. Default `N` is 20.

```sh
ludex sessions -n 50
```

Columns: started (local time), application, full runtime, interactive
runtime, status. Status is the `exit_reason` enum (`terminated`,
`foreground_changed`, `recovered`, `sleep_split`) or `open` for an
in-flight session.

### `ludex backup now`

Takes one snapshot of the live database using SQLite `VACUUM INTO`,
then prunes older snapshots down to the configured retention count.
Prints the path of the new file on stdout.

Safe while the daemon is running — `VACUUM INTO` is consistent under
WAL without blocking the writer.

Snapshots land at `$XDG_DATA_HOME/ludex/backups/ludex-<ISO 8601 UTC>.sqlite`.
Retention defaults to 14 (two weeks of dailies); override the default by
setting `backup_retention_count` in the `settings` table or via the GUI's
Settings page.

### `ludex backup list`

Prints the available snapshots, newest first, with parsed timestamp,
size, and path. Picks up only files matching the
`ludex-<timestamp>Z.sqlite` naming — unrelated SQLite files you may have
in the backup directory are ignored.

### `ludex backup prune [--keep N]`

Prunes to the configured retention count, or to an explicit `--keep N`
override. Clamped at a minimum of 1 so a misconfiguration can never
wipe the full set.

```sh
ludex backup prune           # respect backup_retention_count
ludex backup prune --keep 3  # only keep the three newest
```

### `ludex backup restore <path>`

Atomically replaces `ludex.sqlite` with the contents of a snapshot.

**Refuses to run while `ludex-daemon` is active** — the daemon's open
handles and in-memory WAL state would make the swap unsafe. Stop the
daemon first:

```sh
systemctl --user stop ludex-daemon
ludex backup restore $XDG_DATA_HOME/ludex/backups/ludex-20260423T165537Z.sqlite
systemctl --user start ludex-daemon
```

Under the hood the snapshot is copied to a staging file, opened with
`Database::open` so pending migrations run against the staged copy
(the original backup file is never mutated), the live DB's
`-wal` and `-shm` sidecars are removed, then the staging file is
renamed into place.

### `ludex merge <SRC_ID> <DST_ID>`

Folds one application row into another. Sessions re-parent from `src`
to `dst`, aggregate stats sum (`stat_run_count`, `stat_total_full`,
`stat_total_interactive`) or MAX-fold (`stat_longest_full`),
first-seen / last-played widens to cover both histories, and NULL
metadata slots on `dst` are filled from `src`. Identity slots
(`launcher_type`, `launcher_id`, `product_name`) on `dst` are
preserved. `src` is deleted.

**Refuses to run while `ludex-daemon` is active** — the session
manager holds `application_id`s in memory, and merging the row it
currently has an open session on would leave the manager writing to
a deleted row.

Primary use is post-migration deduplication: a game the Steam source
already tracks as `(steam, <appid>)` and a legacy importer landed as
`(native, <exe_path>)` — one merge per pair collapses them.

```sh
ludex apps list | grep -i "core keeper"
# note the two ids: e.g. 2 (steam) and 5 (native)
systemctl --user stop ludex-daemon
ludex backup now
ludex merge 5 2
systemctl --user start ludex-daemon
```

## Typical workflows

**First-time setup:**

```sh
cargo install --path crates/ludex-cli
ludex doctor                              # verify environment
```

**Before anything destructive** — merge, restore, importer apply:

```sh
ludex backup now                          # snapshot the DB first
```

**Recovering from a bad merge / import / accidental delete:**

```sh
systemctl --user stop ludex-daemon
ludex backup list                         # find the snapshot dated before the mistake
ludex backup restore <that path>
systemctl --user start ludex-daemon
```

**Inspecting history from the terminal:**

```sh
ludex sessions -n 100                     # recent sessions
ludex apps list                           # every tracked app
```
