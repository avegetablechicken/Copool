#!/usr/bin/env python3
"""Build the Rust/Tauri desktop app; on macOS, a locally signed Copool.app.

Signed with the Apple Development certificate in the login keychain, else ad
hoc (see macos_signing.py). Not a distribution release: no Developer ID
signing, notarization or upload.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import macos_signing

ROOT = Path(__file__).resolve().parents[1]
DESKTOP = ROOT / 'desktop'
MACOS = sys.platform == 'darwin'


def run(command, cwd=ROOT, **kwargs):
    print('+ ' + ' '.join(map(str, command)), flush=True)
    result = subprocess.run(command, cwd=cwd, **kwargs)
    if result.returncode:
        raise SystemExit(result.returncode)
    return result


def tauri_cli():
    """`cargo tauri` when installed, otherwise the npm package through npx."""
    if subprocess.run(['cargo', 'tauri', '--version'], cwd=ROOT, capture_output=True).returncode == 0:
        return ['cargo', 'tauri']
    if shutil.which('npx'):
        return ['npx', '--yes', '@tauri-apps/cli@^2']
    raise SystemExit('Tauri CLI not found. Install it with: cargo install tauri-cli --version "^2" --locked')


def check_ui():
    if not shutil.which('node'):
        print('node not found; skipping UI checks', flush=True)
        return
    for path in sorted((DESKTOP / 'ui').glob('*.js')):
        run(['node', '--check', path])
    run(['node', '--test', *sorted((DESKTOP / 'tests').glob('*.test.cjs'))])


def find_bundle(profile, target):
    base = ROOT / 'target' / (target or '') / profile / 'bundle/macos'
    bundles = sorted(base.glob('*.app'))
    if not bundles:
        raise SystemExit(f'No .app bundle found in {base}')
    return bundles[0]


def sign_and_verify(app, build):
    """Signs the widget, then the app; both read their app group from Info.plist."""
    sha1, team = macos_signing.identity()
    group = macos_signing.app_group(team)
    widget = app / 'Contents/PlugIns/CopoolWidgetsMac.appex'
    for bundle, name in ((widget, 'widget'), (app, 'app')):
        if not bundle.exists():
            continue
        run(['plutil', '-replace', macos_signing.APP_GROUP_KEY, '-string', group, bundle / 'Contents/Info.plist'])
        macos_signing.sign(bundle, name, sha1, group, run=run)
    run(['codesign', '--verify', '--deep', '--strict', '--verbose=2', app])
    plist = app / 'Contents/Info.plist'
    executable = subprocess.run(['plutil', '-extract', 'CFBundleExecutable', 'raw', plist],
                                capture_output=True, text=True, check=True).stdout.strip()
    run(['file', app / 'Contents/MacOS' / executable])
    run(['plutil', '-extract', 'CFBundleShortVersionString', 'raw', plist])
    for bundle in (app, widget):
        if not bundle.exists():
            continue
        version = subprocess.run(['plutil', '-extract', 'CFBundleVersion', 'raw', bundle / 'Contents/Info.plist'],
                                 capture_output=True, text=True, check=True).stdout.strip()
        if version != build:
            raise SystemExit(f'{bundle.name} has build {version}, expected {build}')
    return team, group


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--debug', action='store_true', help='debug profile instead of release')
    parser.add_argument('--target', help='Rust target triple, e.g. aarch64-apple-darwin or universal-apple-darwin')
    parser.add_argument('--no-bundle', action='store_true', help='build only the executable (default off macOS)')
    parser.add_argument('--skip-checks', action='store_true', help='skip the UI syntax check and Node tests')
    args = parser.parse_args()
    profile = 'debug' if args.debug else 'release'

    if not args.skip_checks:
        check_ui()

    if args.no_bundle or not MACOS:
        command = ['cargo', 'build', '-p', 'copool-desktop', '--locked']
        if not args.debug:
            command.append('--release')
        if args.target:
            command += ['--target', args.target]
        run(command)
        name = 'copool-desktop.exe' if os.name == 'nt' else 'copool-desktop'
        print(f'Executable: {ROOT / "target" / (args.target or "") / profile / name}')
        return

    # Each build has its own build number (CFBundleVersion), shared by the app
    # and the widget: macOS keeps an extension's list of widgets until its
    # version changes, so widgets added or renamed since would not appear.
    build = time.strftime('%Y%m%d%H%M%S', time.gmtime())
    # The CLI embeds desktop/ui, runs the WidgetKit pre-bundle hook and
    # writes Info.plist; signing is done locally afterwards.
    config = {'bundle': {'active': True, 'macOS': {'bundleVersion': build}}}
    command = [*tauri_cli(), 'build', '--bundles', 'app', '--no-sign', '--ci', '--config', json.dumps(config)]
    if args.debug:
        command.append('--debug')
    if args.target:
        command += ['--target', args.target]
    run(command + ['--', '--locked'], cwd=DESKTOP, env={**os.environ, 'COPOOL_BUNDLE_VERSION': build})

    app = find_bundle(profile, args.target)
    team, group = sign_and_verify(app, build)
    signature = f'signed for team {team}' if team else 'ad-hoc signed'
    print(f'Copool.app (build {build}, {signature}, app group {group}, not notarized): {app}')


if __name__ == '__main__':
    main()
