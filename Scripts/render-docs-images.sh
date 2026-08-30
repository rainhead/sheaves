#!/bin/bash
# Regenerates docs/images from the real views. Nothing appears on screen.
#
# The panel dismisses as soon as attention moves anywhere else, so it cannot be
# screen-captured while anyone is using the Mac. DocumentationImages renders it
# through an offscreen hosting view instead; this script runs that and copies the
# result out of the app's sandbox container, which is the only place the test host
# is allowed to write.
set -euo pipefail
cd "$(dirname "$0")/.."

CONTAINER="$HOME/Library/Containers/com.rainhead.Sheaves/Data/tmp/sheaves-images"
rm -rf "$CONTAINER"

# The TEST_RUNNER_ prefix is required and is stripped on the way through: xcodebuild
# forwards only variables carrying it into the test process. Without it the suite is
# skipped, and a skipped suite still reports TEST SUCCEEDED.
TEST_RUNNER_SHEAVES_RENDER_IMAGES=1 xcodebuild \
  -project Sheaves.xcodeproj -scheme Sheaves -destination 'platform=macOS' \
  test -only-testing:SheavesTests/DocumentationImagesTests \
  >/tmp/sheaves-render.log 2>&1 || { tail -40 /tmp/sheaves-render.log; exit 1; }

shopt -s nullglob
rendered=("$CONTAINER"/*.png)
if [ ${#rendered[@]} -eq 0 ]; then
  echo "No images were rendered — the suite was probably skipped." >&2
  exit 1
fi

mkdir -p docs/images
for f in "${rendered[@]}"; do
  name=$(basename "$f")
  # A raw retina render is ~500 KB; this gets it under 150 KB with text still crisp.
  pngquant --quality 40-85 --speed 1 --strip --force --output "docs/images/$name" "$f"
  oxipng -o 4 --strip safe "docs/images/$name" >/dev/null
  printf '%s  %s\n' "$(du -h "docs/images/$name" | cut -f1)" "docs/images/$name"
done
