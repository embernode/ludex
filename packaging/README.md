# packaging

Distribution artefacts for ludex.

## Building and installing from source (local, unpackaged)

For running an in-development build long-term — e.g. tracking `main` on
your own machine — without going through `makepkg` / GitHub Releases.
Everything installs under your home directory; nothing touches `/usr`, so
it never collides with a packaged install (uninstall the package first if
you have one, to avoid two copies on `PATH`).

### Daemon + CLI

```sh
cargo install --path crates/ludex-daemon   # -> ~/.cargo/bin/ludex-daemon
cargo install --path crates/ludex-cli      # -> ~/.cargo/bin/ludex
```

Then enable the background service as described in the
[`ludex-daemon.service`](#ludex-daemonservice-systemd---user-unit)
section below — the stock unit already points `ExecStart` at
`~/.cargo/bin/ludex-daemon`, so no edit is needed for a source build.

### GUI

The GUI binary embeds the built SvelteKit frontend, so it has to be built
through Tauri — a plain `cargo build` skips the frontend step. These are
the same commands the `PKGBUILD` runs, just installed into your home
instead of `/usr`:

```sh
cd app
pnpm install
pnpm exec tauri build --no-bundle          # -> ../target/release/ludex-gui
```

Install the binary, desktop entry, and icon into your user directories
(`~/.local/bin` must be on your `PATH` — it is on a default systemd user
session):

```sh
install -Dm755 ../target/release/ludex-gui ~/.local/bin/ludex-gui
install -Dm644 ../packaging/net.ludex.gui.desktop \
    ~/.local/share/applications/net.ludex.gui.desktop
install -Dm644 src-tauri/icons/icon_light.png \
    ~/.local/share/icons/hicolor/256x256/apps/net.ludex.gui.png
update-desktop-database ~/.local/share/applications 2>/dev/null || true
gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor 2>/dev/null || true
```

Launch it from your app menu (it registers as "ludex") or run `ludex-gui`.
If the taskbar icon doesn't resolve, see [If the icon still doesn't
show](#if-the-icon-still-doesnt-show).

### Updating a source install

Re-run the relevant build. For the daemon, reinstall and cycle the
service so the running process picks up the new binary:

```sh
cargo install --path crates/ludex-daemon && systemctl --user restart ludex-daemon
```

For the GUI, rebuild and overwrite the installed binary, then reopen the
window:

```sh
cd app && pnpm exec tauri build --no-bundle \
    && install -Dm755 ../target/release/ludex-gui ~/.local/bin/ludex-gui
```

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

The stock unit points at `%h/.cargo/bin/ludex-daemon` — where the
`cargo install` above puts the daemon, next to the `ludex` CLI. If
you install system-wide (as the `PKGBUILD` in this directory does —
it lands the daemon at `/usr/bin/ludex-daemon`), a unit with the
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
`StartupWMClass=ludex-gui` in the desktop entry matches the Wayland
app-id wry actually sets on the toplevel — the **binary basename**,
not the `net.ludex.gui` bundle identifier (see the comment in
`net.ludex.gui.desktop` and [If the icon still doesn't
show](#if-the-icon-still-doesnt-show)). The daemon's user unit lives
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
