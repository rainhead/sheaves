#!/bin/bash
# Renders the Mac app icon's PNGs from icons/com.rainhead.Sheaves.svg, the same
# tile the GNOME app installs. Run after changing that file.
#
# macOS draws an app icon's body at 824 of 1024 pixels, centred, so each size is
# the tile scaled to that body and padded out to the full square.
# Needs rsvg-convert and ImageMagick: brew install librsvg imagemagick
set -euo pipefail
cd "$(dirname "$0")/.."

source=icons/com.rainhead.Sheaves.svg
out=Apps/Mac/Resources/Assets.xcassets/AppIcon.appiconset
mkdir -p "$out"

# The tile fills 112 of the SVG's 128 units.
for size in 16 32 64 128 256 512 1024; do
  render=$(( size * 824 * 128 / (1024 * 112) ))
  rsvg-convert -w "$render" -h "$render" "$source" \
    | magick - -background none -gravity center -extent "${size}x${size}" "$out/icon_$size.png"
done

cat > "$out/Contents.json" <<'JSON'
{
  "images" : [
    { "filename" : "icon_16.png", "idiom" : "mac", "scale" : "1x", "size" : "16x16" },
    { "filename" : "icon_32.png", "idiom" : "mac", "scale" : "2x", "size" : "16x16" },
    { "filename" : "icon_32.png", "idiom" : "mac", "scale" : "1x", "size" : "32x32" },
    { "filename" : "icon_64.png", "idiom" : "mac", "scale" : "2x", "size" : "32x32" },
    { "filename" : "icon_128.png", "idiom" : "mac", "scale" : "1x", "size" : "128x128" },
    { "filename" : "icon_256.png", "idiom" : "mac", "scale" : "2x", "size" : "128x128" },
    { "filename" : "icon_256.png", "idiom" : "mac", "scale" : "1x", "size" : "256x256" },
    { "filename" : "icon_512.png", "idiom" : "mac", "scale" : "2x", "size" : "256x256" },
    { "filename" : "icon_512.png", "idiom" : "mac", "scale" : "1x", "size" : "512x512" },
    { "filename" : "icon_1024.png", "idiom" : "mac", "scale" : "2x", "size" : "512x512" }
  ],
  "info" : { "author" : "xcode", "version" : 1 }
}
JSON
echo "rendered into $out"
