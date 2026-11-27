"""macOS steps of the shared desktop build: a locally signed Copool.app."""
import json
import os
import subprocess
import time

import macos_signing
from build_common import DESKTOP, cargo_args, cargo_build, output_dir, run, tauri_cli


def add_arguments(parser):
    pass


def prepare(args):
    pass


def find_bundle(args):
    base = output_dir(args) / 'bundle/macos'
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


def build(args):
    if args.no_bundle:
        print(f'Executable: {cargo_build(args)}')
        return
    # Each build has its own build number (CFBundleVersion), shared by the app
    # and the widget: macOS keeps an extension's list of widgets until its
    # version changes, so widgets added or renamed since would not appear.
    number = time.strftime('%Y%m%d%H%M%S', time.gmtime())
    # The CLI embeds desktop/ui, runs the WidgetKit pre-bundle hook and
    # writes Info.plist; signing is done locally afterwards.
    config = {'bundle': {'active': True, 'macOS': {'bundleVersion': number}}}
    command = [*tauri_cli(), 'build', '--bundles', 'app', '--no-sign', '--ci', '--config', json.dumps(config)]
    if args.debug:
        command.append('--debug')
    if args.target:
        command += ['--target', args.target]
    run(command + ['--', *cargo_args(args)], cwd=DESKTOP, env={**os.environ, 'COPOOL_BUNDLE_VERSION': number})

    app = find_bundle(args)
    team, group = sign_and_verify(app, number)
    signature = f'signed for team {team}' if team else 'ad-hoc signed'
    print(f'Copool.app (build {number}, {signature}, app group {group}, not notarized): {app}')
