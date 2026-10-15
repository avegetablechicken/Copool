#!/usr/bin/env python3
"""Tauri's macOS-only pre-bundle hook: build the native WidgetKit extension."""
import json
import os
from pathlib import Path
import subprocess
import sys
import macos_signing
ROOT=Path(__file__).resolve().parents[1]
if sys.platform != 'darwin' or os.environ.get('TAURI_ENV_PLATFORM', 'darwin') not in ('darwin','macos'):
    raise SystemExit(0)
config=json.loads((ROOT/'desktop/tauri.conf.json').read_text())
# The app's build number, from build_rust_desktop.py.
build=os.environ.get('COPOOL_BUNDLE_VERSION',config['version'])
output=ROOT/'.build/rust-widget'
output.mkdir(parents=True,exist_ok=True)
arch={'aarch64':'arm64','x86_64':'x86_64','universal':'arm64 x86_64'}.get(os.environ.get('TAURI_ENV_ARCH',''),os.uname().machine)
sha1,team=macos_signing.identity()
group=macos_signing.app_group(team)
command=['xcodebuild','-project','desktop/widget/CopoolWidgets.xcodeproj','-target','CopoolWidgetsMac','-configuration','Release','-sdk','macosx','CODE_SIGNING_ALLOWED=NO','CODE_SIGNING_REQUIRED=NO',f'PRODUCT_BUNDLE_IDENTIFIER={config["identifier"]}.widgets',f'MARKETING_VERSION={config["version"]}',f'CURRENT_PROJECT_VERSION={build}',f'COPOOL_APP_GROUP={group}',f'ARCHS={arch}','ONLY_ACTIVE_ARCH=NO',f'CONFIGURATION_BUILD_DIR={output}/Release',f'OBJROOT={output}/Objects',f'SYMROOT={output}/Products',f'CLANG_MODULE_CACHE_PATH={output}/ModuleCache.noindex',f'SWIFT_MODULE_CACHE_PATH={output}/ModuleCache.noindex','build']
log=output/'build.log'
with log.open('w') as stream:
    result=subprocess.run(command,cwd=ROOT,stdout=stream,stderr=subprocess.STDOUT)
if result.returncode:
    print('\n'.join(log.read_text(errors='replace').splitlines()[-60:]),file=sys.stderr)
    raise SystemExit(result.returncode)
bundle=output/'Release/CopoolWidgetsMac.appex'
macos_signing.sign(bundle,'widget',sha1,group,run=lambda command: subprocess.run(command,check=True))
print(f'WidgetKit extension: {bundle}')
