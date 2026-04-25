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

## Updating the daemon during development

`cargo install --path crates/ludex-daemon` atomically replaces the
binary on disk, but the already-running systemd-managed process
keeps its old mapping — so rebuilds don't take effect until the
service cycles. The one-liner:

```sh
cargo install --path crates/ludex-daemon \
  && systemctl --user restart ludex-daemon
```

On restart the daemon closes any in-flight sessions cleanly with
`exit_reason = 'terminated'`, applies any pending SQLite
migrations, and resumes — no data loss, but you do pay a
session-boundary's worth of churn on each cycle.

For tight iteration on the daemon itself, **stop the service and
run the daemon in the foreground** instead:

```sh
systemctl --user stop ludex-daemon
cargo run -p ludex-daemon          # Ctrl-C to exit
# ... iterate ...
systemctl --user start ludex-daemon
```

Foreground mode gives you inline stderr logs and a faster edit-
compile loop; the service is the better answer once you're
confident a build is stable.

Logs go to the user journal by default. `LUDEX_LOG=debug` tuned
via `systemctl --user edit ludex-daemon` (it creates a drop-in)
if you need finer verbosity:

```ini
[Service]
Environment="LUDEX_LOG=debug"
```

## `PKGBUILD` (Arch package — daemon + CLI + GUI + unit + icons)

Builds and installs everything ludex ships from the working tree
of this checkout: the daemon, the CLI, the Tauri GUI, the
`net.ludex.gui.desktop` entry, the hicolor icon set, and a
system-path systemd `--user` unit pointing at `/usr/bin/ludex-daemon`.

```sh
cd packaging
makepkg -si        # builds the package and installs via pacman
```

`makepkg` will warn about the empty `source=()` — that's expected;
this is an in-repo PKGBUILD that builds from `$startdir/..` rather
than a fetched tarball. (A separate `-git` PKGBUILD targeting AUR
with a real git source is on the roadmap.)

After install, the GUI's taskbar icon resolves correctly because
`StartupWMClass=net.ludex.gui` in the desktop entry matches the
Wayland app-id Tauri 2 sets via GTK. The daemon's user unit lives
at `/usr/lib/systemd/user/ludex-daemon.service` so every account
can enable it without copying anything:

```sh
systemctl --user enable --now ludex-daemon
```

To remove: `pacman -Rns ludex` cleans up every file in one shot.

### Updating after a code change

`makepkg -si` again — `--locked` on the cargo invocation rebuilds
incrementally against the workspace's existing `target/`, so
subsequent runs are fast.

### If the icon still doesn't show

Confirm the running window's app-id actually matches the
`StartupWMClass=net.ludex.gui` line in the `.desktop` file. On
Plasma 6 Wayland: open the **System Monitor** widget, right-click
the ludex process, **Properties** → **App ID**. Adjust the
`StartupWMClass` in the desktop entry if it differs (and rebuild
the package).

## (planned) AUR PKGBUILD

A `-git` variant pulling from the public GitHub source rather
than the working tree is on the roadmap for an AUR submission.
Until then the in-repo PKGBUILD above covers single-machine
installs from a checked-out clone.
