# assets

Brand source files for ludex. The master is `logo.svg`;
`logo_<size>_<theme>.png` rasterisations exist as a convenience for
previewing and for the README's hero image at sizes where SVG
rendering is awkward (e.g. small avatars).

## The mark

A clock ring with four ticks cut out of a heavy stroke, and a play
triangle inside it. Drawn on a 32x32 grid so strokes land on whole
pixels at 16, 32 and 64; the dash pattern divides the ring's
circumference to within 0.002 units, which is what keeps the four
gaps on the compass points (they land within 0.05 degrees of them).

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
  neutral ring, plus the three sizes Tauri bundles (`32x32.png`,
  `128x128.png`, `128x128@2x.png`, the last of which is 256px). A
  desktop-file icon is not recoloured by the panel, so this one has to
  read against both backgrounds by itself; the neutral ring plus the
  green triangle does that without needing two files. The PNG and the
  SVG are what the PKGBUILD installs under `/usr/share/icons/`.

`icon.svg` is a copy of this master kept for reference. Nothing loads
it — not the bundle list, not the PKGBUILD, not the tray.

```sh
ink '#000000' /tmp/black.svg; ink '#ffffff' /tmp/white.svg
tri() { sed "s|fill=\"#6ec46e\"|fill=\"$1\"|" "$2" > "$3"; }
tri '#000000' /tmp/black.svg /tmp/tray_black.svg
tri '#ffffff' /tmp/white.svg /tmp/tray_white.svg
cd ../app/src-tauri/icons
rsvg-convert -w 256 -h 256 /tmp/tray_black.svg -o icon.png
rsvg-convert -w 256 -h 256 /tmp/tray_white.svg -o icon_light.png
rsvg-convert -w 256 -h 256 /tmp/black.svg      -o icon_active.png
rsvg-convert -w 256 -h 256 /tmp/white.svg      -o icon_active_light.png
ink '#7a838a' icon_app.svg
rsvg-convert -w 256 -h 256 icon_app.svg -o icon_app.png
rsvg-convert -w 32  -h 32  icon_app.svg -o 32x32.png
rsvg-convert -w 128 -h 128 icon_app.svg -o 128x128.png
rsvg-convert -w 256 -h 256 icon_app.svg -o '128x128@2x.png'
```

Regenerate **all four** application PNGs together. Tauri picks the
embedded window icon by taking the first `.png` in `tauri.conf.json`'s
bundle list, which is `32x32.png` — not `icon_app.png` — so refreshing
only the obvious one leaves the window wearing the old mark.

## A known limit

The neutral ring clears the 3:1 non-text contrast threshold against
every realistic panel (about 3.4:1 on Breeze Light, 3.2:1 on Breeze
Dark, 5:1 on the app's own dark page), but not by much, and it
disappears against a mid-grey backdrop — an adaptive-transparency
panel over a mid-tone wallpaper. The green triangle is the weaker
half: around 1.9:1 on any light panel, so on a light theme the icon
reads mostly as a ring. The green is fixed by the brand, so this is a
trade rather than a bug; revisit the ring value, not the green, if it
ever needs more separation.
