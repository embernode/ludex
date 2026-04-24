# packaging

Distribution artefacts for ludex.

## `ludex-daemon.service` (systemd `--user` unit)

First install the daemon binary — the stock unit expects it at
`%h/.cargo/bin/ludex-daemon`:

```sh
cargo install --path crates/ludex-daemon
```

Then drop the unit in, reload, enable:

```sh
mkdir -p ~/.config/systemd/user
cp packaging/ludex-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now ludex-daemon
```

Verify it's up:

```sh
systemctl --user status ludex-daemon
journalctl --user -u ludex-daemon -f        # follow the log
```

The stock unit points at `%h/.cargo/bin/ludex-daemon` — the
location `cargo install --path crates/ludex-cli` documented in
the README uses for the daemon's sibling binary. If you install
system-wide (e.g. via an AUR PKGBUILD that lands at
`/usr/bin/ludex-daemon`), drop a unit with the `ExecStart` path
adjusted into `/usr/lib/systemd/user/` and systemd will auto-
discover it for every user.

To stop for a merge / restore / import that needs exclusive
database access:

```sh
systemctl --user stop ludex-daemon
ludex merge <src> <dst>
systemctl --user start ludex-daemon
```

Logs go to the user journal by default. `LUDEX_LOG=debug` tuned
via `systemctl --user edit ludex-daemon` (it creates a drop-in)
if you need finer verbosity:

```ini
[Service]
Environment="LUDEX_LOG=debug"
```

## (planned) `PKGBUILD`

An AUR PKGBUILD shipping `ludex-daemon`, `ludex`, the GUI bundle,
and this unit file is on the post-M6 roadmap. Not yet present.
