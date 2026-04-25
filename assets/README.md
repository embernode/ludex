# assets

Brand source files for ludex. The master is `logo.svg` (Inkscape);
`logo_<size>_<theme>.png` rasterisations exist as a convenience
for previewing and for the README's hero image at sizes where
SVG rendering is awkward (e.g. small avatars).

## What ships from here

* `logo.svg` — the canonical master. Edit here when the brand
  evolves; regenerate the rasterisations from it.
* `logo_{16,32,48,64,128,256}_{dark,light}.png` — pre-rendered
  sizes with two colour variants. `_dark` is the dark silhouette
  for use on light backgrounds; `_light` is white for dark
  backgrounds. Plasma's default theme is dark, so the `_light`
  variants are what KDE picks up in the taskbar.

## Relationship to runtime icons

The Tauri runtime icon set lives separately under
`app/src-tauri/icons/`. Those files are what the GUI binary
loads at runtime (window icon, tray icon — themed based on the
detected system theme) and what the PKGBUILD ships under
`/usr/share/icons/hicolor/`. They are *derived* from the assets
here; if you regenerate `app/src-tauri/icons/icon_light.png`
from a new `logo.svg`, run `pnpm exec tauri icon` from `app/`
to refresh the size variants Tauri bundles.

The current single-colour silhouettes are visible-on-one-theme
only. A multi-colour brand mark that reads on both light and
dark backgrounds is on the roadmap — see `docs/roadmap.md` →
"GUI backlog".
