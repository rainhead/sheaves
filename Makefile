XCODEBUILD := xcodebuild -project Sheaves.xcodeproj -scheme Sheaves -destination 'platform=macOS'

.PHONY: generate build run test logs

generate:
	xcodegen generate

build: generate
	$(XCODEBUILD) build

run: build
	open "$$($(XCODEBUILD) -showBuildSettings 2>/dev/null | awk '/BUILT_PRODUCTS_DIR/ { print $$3; exit }')/Sheaves.app"

test: generate
	swift test --package-path Packages/SheavesCore
	$(XCODEBUILD) test

logs:
	/usr/bin/log show --last 5m --info --predicate 'subsystem == "com.rainhead.Sheaves"'
