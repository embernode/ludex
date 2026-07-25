# assets

Brand source files for ludex. The master is `logo.svg`;
`logo_<size>_<theme>.png` rasterisations exist as a convenience for
previewing and for the README's hero image at sizes where SVG
rendering is awkward (e.g. small avatars).

## The mark

A clock ring with four ticks cut out of a heavy stroke, and a play
triangle inside it. Drawn on a 32x32 grid so strokes land on whole
pixels at 16, 32 and 64; the dash pattern divides the ring's
circumference exactly, which is what keeps the four gaps on the
compass points.

The ring is `currentColor` — a host that themes text themes the mark
with it, which is how the GUI's header mark follows the colour scheme.
Green appears only on the triangle, and it is a **fixed** brand green:
it deliberately does not follow the accent picker in Settings, because
the mark shouldn't change colour when the user retints the interface.

## What ships from here

* `logo.svg` — the canonical master. Edit here when the brand evolves,
  then regenerate everything below.
* `logo_{16,32,48,64,128,256}_{dark,light}.png` — pre-rendered sizes.
  `_dark` is the dark-ink mark for light backgrounds; `_light` is the
  white-ink mark for dark ones. Both keep the green triangle.

## Regenerating

`currentColor` has no meaning in a PNG, so each variant substitutes a
concrete ring colour first:

```sh
ink() { sed "s|stroke=\"currentColor\"|stroke=\"$1\"|" logo.svg > "$2"; }
ink '#000000' /tmp/dark.svg
ink '#ffffff' /tmp/light.svg
for s in 16 32 48 64 128 256; do
  rsvg-convert -w $s -h $s /tmp/dark.svg  -o "logo_${s}_dark.png"
  rsvg-convert -w $s -h $s /tmp/light.svg -o "logo_${s}_light.png"
done
```

## Relationship to runtime icons

The Tauri runtime icon set lives separately under
`app/src-tauri/icons/` and is derived from this master. Two groups:

* **Tray** — `icon.png` / `icon_light.png` (idle, black and white ink)
  and `icon_active.png` / `icon_active_light.png` (same rings, green
  triangle). The daemon-idle pair is monochrome so it behaves like a
  panel icon; playing swaps exactly one thing, the triangle fill. The
  GUI picks the ink from the desktop's colour scheme at runtime, via
  the freedesktop appearance portal.
* **Application** — `icon_app.png` and `icon_app.svg`, the mark with a
  neutral ring. A desktop-file icon is not recoloured by the panel, so
  this one has to read against both backgrounds by itself; the neutral
  ring plus the green triangle does that without needing two files.
  These are what the PKGBUILD installs under `/usr/share/icons/`, and
  what `tauri.conf.json` bundles.

Regenerate those the same way, substituting `#000000`, `#ffffff` and
`#7a838a` for the ring, and `#6ec46e` or the ring colour for the
triangle depending on whether the variant is the active one.
