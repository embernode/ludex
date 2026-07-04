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
system-wide (as the `PKGBUILD` in this directory does — it lands
the daemon at `/usr/bin/ludex-daemon`), a unit with the
`ExecStart` path adjusted goes into `/usr/lib/systemd/user/` and
systemd auto-discovers it for every user.

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
than a fetched tarball. There's no AUR package — the prebuilt
`.pkg.tar.zst` is published on GitHub Releases instead (see
[Publishing a release](#publishing-a-release)).

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

If the panel / Kickoff still renders a black silhouette after a
clean install on KDE Plasma 6, it's almost always one of two
things:

1. **Plasma's icon cache is stale.** `pacman -U` updates the
   PNG on disk but Plasma keeps the previous render in
   `~/.cache/icon-cache.kcache` plus its in-memory copy.
   Force a refresh:

   ```sh
   rm -f ~/.cache/icon-cache.kcache
   kbuildsycoca6 --noincremental
   kquitapp6 plasmashell && setsid plasmashell >/dev/null 2>&1 &
   ```

   Then close + reopen the GUI window so its taskbar entry
   re-resolves against the fresh icon.

2. **The window's app-id doesn't match the `.desktop`'s
   `StartupWMClass`.** Tauri 2 / wry on Wayland sets the
   xdg-toplevel app-id from the binary basename
   (`ludex-gui`), not the bundle identifier
   (`net.ludex.gui`) — so the `.desktop` declares
   `StartupWMClass=ludex-gui` to match. Confirm what KDE
   actually sees:

   ```sh
   qdbus6 org.kde.KWin /KWin org.kde.KWin.queryWindowInfo
   # click the GUI window when prompted; check `desktopFile:`
   ```

   `desktopFile: ludex-gui` is correct. If it's something
   else, adjust `StartupWMClass` to match and rebuild.

## Publishing a release

Prebuilt Arch packages ship via
[GitHub Releases](https://github.com/embernode/ludex/releases);
there's no AUR package. Pushing a `vX.Y.Z` tag triggers
`.github/workflows/release.yml`, which runs this directory's
`makepkg` in an `archlinux` container and publishes the release
with the `.pkg.tar.zst` attached.

1. Bump the version in all four pinned locations — they must
   agree:

   - `Cargo.toml` (workspace `[workspace.package] version`)
   - `app/package.json` (`version`)
   - `app/src-tauri/tauri.conf.json` (`version`)
   - `packaging/PKGBUILD` (`pkgver`)

2. Commit the bump, then tag and push — the tag push does the
   rest. Use an annotated tag: its message becomes the release
   notes verbatim (a lightweight tag falls back to GitHub's
   auto-generated notes):

   ```sh
   git commit -am "release: X.Y.Z"
   git tag -a vX.Y.Z        # write the release notes in the editor
   git push origin main --tags
   ```

The manual fallback (CI down, or a tag pushed before the workflow
existed) is the same flow by hand — `cd packaging && makepkg -f`,
then `gh release create` with the artifact attached — or the
workflow's `workflow_dispatch` trigger, which builds the
dispatched ref and releases it under the PKGBUILD's `pkgver`
(the tag must already exist).
