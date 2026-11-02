#!/usr/bin/env bash
# Builds the Rust core for the Mac app: a universal static library wrapped in an
# XCFramework, plus the Swift that UniFFI generates to call it. Both land inside
# Packages/SheavesCore, which is where SwiftPM requires a binary target to live,
# and both are git-ignored, so run this before building the Swift package or the
# app. `make build` and `make test` do.
#
#     Scripts/build-xcframework.sh
set -euo pipefail

cd "$(dirname "$0")/.."

crate=sheaves_core
module=${crate}FFI
package=Packages/SheavesCore
framework=$package/Frameworks/$module.xcframework
bindings=$package/Sources/SheavesCore/Generated
staging=target/xcframework

# Mac only, although Package.swift also names iOS: there is no iOS app yet. When
# there is, add the aarch64-apple-ios and aarch64-apple-ios-sim targets here and
# in rust-toolchain.toml, and pass each library to -create-xcframework below.
#
# Release even for Debug builds of the app: the core is small, cargo caches it,
# and one profile means one artifact to reason about.
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --locked --release --lib -p sheaves-core --target "$target"
done

rm -rf "$staging"
mkdir -p "$staging/lib" "$staging/headers" "$staging/swift"

# One library for both architectures, because a release build of the app is
# universal and an XCFramework holds one library per platform, not per arch.
lipo -create \
  "target/aarch64-apple-darwin/release/lib$crate.a" \
  "target/x86_64-apple-darwin/release/lib$crate.a" \
  -output "$staging/lib/lib$crate.a"

# The generator reads the interface from metadata compiled into the library, so
# the Swift always describes the library it ships with. It is built from the same
# pinned uniffi version (crates/uniffi-bindgen), which the generated Swift checks
# at load time.
cargo run --locked --quiet -p uniffi-bindgen -- \
  --swift-sources --headers --modulemap \
  --module-name "$module" --modulemap-filename module.modulemap \
  "target/aarch64-apple-darwin/release/lib$crate.a" "$staging/headers"
mv "$staging/headers/$crate.swift" "$staging/swift/"

rm -rf "$framework"
xcodebuild -create-xcframework \
  -library "$staging/lib/lib$crate.a" \
  -headers "$staging/headers" \
  -output "$framework" >/dev/null

mkdir -p "$bindings"
cp "$staging/swift/$crate.swift" "$bindings/"

echo "Built $framework and $bindings/$crate.swift"
