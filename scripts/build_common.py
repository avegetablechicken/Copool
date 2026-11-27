"""Steps shared by the desktop build entry point and its platform modules."""
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DESKTOP = ROOT / 'desktop'


def run(command, cwd=ROOT, **kwargs):
    print('+ ' + ' '.join(map(str, command)), flush=True)
    result = subprocess.run(list(map(str, command)), cwd=cwd, **kwargs)
    if result.returncode:
        raise SystemExit(result.returncode)
    return result


def tauri_cli():
    """`cargo tauri` when installed, otherwise the npm package through npx."""
    if subprocess.run(['cargo', 'tauri', '--version'], cwd=ROOT, capture_output=True).returncode == 0:
        return ['cargo', 'tauri']
    if shutil.which('npx'):
        node = shutil.which('node')
        if os.name == 'nt' and node:
            npx_cli = Path(node).parent / 'node_modules/npm/bin/npx-cli.js'
            if npx_cli.exists():
                return ['node', str(npx_cli), '--yes', '@tauri-apps/cli@^2']
        return [shutil.which('npx'), '--yes', '@tauri-apps/cli@^2']
    raise SystemExit('Tauri CLI not found. Install it with: cargo install tauri-cli --version "^2" --locked')


def profile(args):
    return 'debug' if args.debug else 'release'


def target_dir():
    """Cargo's output directory: `CARGO_TARGET_DIR` (made absolute by the entry
    point) or `target`."""
    return Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target')


def output_dir(args):
    """Where this build's products are: under the target triple and the profile."""
    return target_dir() / (args.target or '') / profile(args)


def cargo_args(args):
    """Arguments for Cargo itself, shared by `cargo build` and the Tauri CLI."""
    return ['--locked'] + (['--offline'] if args.offline else [])


def cargo_build(args, prefix=()):
    """Builds the desktop executable and returns its path."""
    command = [*prefix, 'cargo', 'build', '-p', 'copool-desktop', *cargo_args(args)]
    if not args.debug:
        command.append('--release')
    if args.target:
        command += ['--target', args.target]
    run(command)
    return output_dir(args) / ('copool-desktop.exe' if os.name == 'nt' else 'copool-desktop')
