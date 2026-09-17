# Copool Repository Rules

## Release Terminology

- When the user says `提交 TestFlight 审核`, `提交外部测试审核`, or similar wording, always treat it as `submit the build for TestFlight external beta review`, not App Store review.
- Do not start App Store submission work unless the user explicitly says `App Store 审核`, `提交 App Store`, `提交苹果正式审核`, or equivalent.
- For release tasks that mention both TestFlight and review in the same request, default to:
  1. upload to TestFlight
  2. add tester groups if needed
  3. submit TestFlight external beta review
  Only start App Store submission if the user separately and explicitly asks for it.

## Local macOS App Bundle Build

- For requests to build the application locally, use the following verified workflow from the repository root. Build the Swift/Xcode `Copool` project; do not use the unrelated Electron scripts in `package.json`.
- Default to a Release bundle with local ad-hoc signing. The current machine lacks the Mac App Development provisioning profiles for `com.alick.copool` and `com.alick.copool.widgets`, so disable Xcode signing during the build. Do not repeat provisioning discovery or enable provisioning updates for an ordinary local build.
- Xcode may require execution outside the sandbox to access system build services and caches; use the execution tool's approval mechanism when needed.

```bash
xcodebuild -project Copool.xcodeproj -scheme Copool \
  -configuration Release -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath .build/app-bundle \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO \
  build > /tmp/copool-app-bundle-build.log 2>&1
```

Wait for a successful exit and `BUILD SUCCEEDED` in the log before signing. On failure, inspect `/tmp/copool-app-bundle-build.log` for errors.

Sign the widget extension first, then the app, and verify the completed bundle:

```bash
codesign --force --sign - --entitlements CopoolWidgetsMac.entitlements \
  .build/app-bundle/Build/Products/Release/Copool.app/Contents/PlugIns/CopoolWidgetsMac.appex
codesign --force --sign - \
  .build/app-bundle/Build/Products/Release/Copool.app
codesign --verify --deep --strict --verbose=2 \
  .build/app-bundle/Build/Products/Release/Copool.app
file .build/app-bundle/Build/Products/Release/Copool.app/Contents/MacOS/Copool
plutil -extract CFBundleShortVersionString raw \
  .build/app-bundle/Build/Products/Release/Copool.app/Contents/Info.plist
```

- Deliver `.build/app-bundle/Build/Products/Release/Copool.app` as a clickable absolute path. This workflow currently produces a universal arm64/x86_64 executable; verify the architectures and version instead of assuming them.
- This is a locally ad-hoc-signed build, not a notarized distribution release. Do not apply `Copool.release.entitlements` to the ad-hoc-signed main app. Provisioning-dependent capabilities are not validated by signature verification alone.
- An ordinary build does not include installing, launching, replacing an installed app, committing, uploading, or publishing. Perform those only when requested.
- For an explicitly requested distribution release, follow `docs/release-macos.md` and `scripts/release_macos.sh` for Developer ID signing and notarization instead.
