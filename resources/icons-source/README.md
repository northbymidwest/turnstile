# Icon sources

`icon-openrct2.png` and `icon-openloco.png` here are the originals as imported from
upstream OpenLauncher, 1024x1024. They are kept so the full resolution stays in the
repository and a larger rendering is possible later without re-importing.

The copies one directory up, in `resources/`, are what the app actually loads and what
`bundle.sh` ships. They are 40x40, which is the exact size the sidebar draws them at:
`crates/app/src/views/sidebar.rs` constrains the image view to 20x20 points, and macOS
displays are 1x or 2x, never 3x, so 40 pixels is a 1:1 match on Retina and a clean 2:1
downscale on a non-Retina display.

Shipping the 1024x1024 originals meant carrying 655 times the pixels that ever reached
the screen, about 300 KB of the bundle. Lossless optimization of those originals was
worth about 15%; this is worth about 96%.

To regenerate the shipped copies after changing a source:

```
for n in icon-openrct2 icon-openloco; do
  magick resources/icons-source/$n.png -filter Lanczos -resize 40x40 -strip resources/$n.png
  oxipng -o max --zopfli --strip all -a -q resources/$n.png
done
```

Upstream is MIT licensed; see `LICENSE-OpenLauncher` for the attribution these icons
carry. They are not covered by this repository's 0BSD license.
