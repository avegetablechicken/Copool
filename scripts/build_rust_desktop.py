#!/usr/bin/env python3
"""Build the Rust/Tauri desktop app, packaged for this platform.

macOS: a locally signed Copool.app (macos_build.py), signed with the Apple
Development certificate in the login keychain, else ad hoc (macos_signing.py).
Windows: an unsigned NSIS installer (windows_build.py). Linux: the executable,
checked for its libraries (linux_build.py). Not a distribution release: no
Developer ID signing, notarization or upload.
"""
import argparse
import os
import shutil
import sys
from pathlib import Path

from build_common import DESKTOP, run


def platform_module():
    if sys.platform == 'darwin':
        import macos_build as module
    elif os.name == 'nt':
        import windows_build as module
    else:
        import linux_build as module
    return module


def check_ui():
    if not shutil.which('node'):
        print('node not found; skipping UI checks', flush=True)
        return
    for path in sorted((DESKTOP / 'ui').glob('*.js')):
        run(['node', '--check', path])
    run(['node', '--test', *sorted((DESKTOP / 'tests').glob('*.test.cjs'))])


def main():
    platform = platform_module()
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--debug', action='store_true', help='debug profile instead of release')
    parser.add_argument('--target', help='Rust target triple, e.g. aarch64-apple-darwin or universal-apple-darwin')
    parser.add_argument('--no-bundle', action='store_true', help='build only the executable (always so on Linux)')
    parser.add_argument('--skip-checks', action='store_true', help='skip the UI syntax check and Node tests')
    parser.add_argument('--offline', action='store_true', help='use only the crates Cargo has already fetched')
    platform.add_arguments(parser)
    args = parser.parse_args()
    # Cargo and the Tauri CLI run in different directories; both must read
    # a relative CARGO_TARGET_DIR as the caller meant it.
    if os.environ.get('CARGO_TARGET_DIR'):
        os.environ['CARGO_TARGET_DIR'] = str(Path(os.environ['CARGO_TARGET_DIR']).expanduser().resolve())

    platform.prepare(args)
    if not args.skip_checks:
        check_ui()
    platform.build(args)


if __name__ == '__main__':
    main()
