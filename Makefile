XCODEBUILD := xcodebuild -project Sheaves.xcodeproj -scheme Sheaves -destination 'platform=macOS'

.PHONY: core generate build run test logs package

# The Rust core, built into the Swift package; see Scripts/build-xcframework.sh.
core:
	Scripts/build-xcframework.sh

generate:
	xcodegen generate

build: core generate
	$(XCODEBUILD) build

# `open` on a running app only activates the old instance, so quit it first or
# a rebuild launches yesterday's binary.
run: build
	@pkill -x Sheaves && sleep 1 || true
	open "$$($(XCODEBUILD) -showBuildSettings 2>/dev/null | awk '/BUILT_PRODUCTS_DIR/ { print $$3; exit }')/Sheaves.app"

test: core generate
	cargo test --locked
	swift test --package-path Packages/SheavesCore
	$(XCODEBUILD) test

logs:
	/usr/bin/log show --last 5m --info --predicate 'subsystem == "com.rainhead.Sheaves"'

# What a release ships, built the way CI builds it — ad-hoc signed regardless of
# any Config/Local.xcconfig on this machine.
package:
	Scripts/package-release.sh
