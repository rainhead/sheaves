XCODEBUILD := xcodebuild -project Sheaves.xcodeproj -scheme Sheaves -destination 'platform=macOS'

.PHONY: generate build run test logs

generate:
	xcodegen generate

build: generate
	$(XCODEBUILD) build

# `open` on a running app only activates the old instance, so quit it first or
# a rebuild launches yesterday's binary.
run: build
	@pkill -x Sheaves && sleep 1 || true
	open "$$($(XCODEBUILD) -showBuildSettings 2>/dev/null | awk '/BUILT_PRODUCTS_DIR/ { print $$3; exit }')/Sheaves.app"

test: generate
	swift test --package-path Packages/SheavesCore
	$(XCODEBUILD) test

logs:
	/usr/bin/log show --last 5m --info --predicate 'subsystem == "com.rainhead.Sheaves"'
