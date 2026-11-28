"""Windows steps of the shared desktop build: Visual Studio's environment and an
unsigned NSIS installer."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

from build_common import DESKTOP, cargo_args, cargo_build, output_dir, run, target_dir, tauri_cli


def add_arguments(parser):
    pass


def prepare(args):
    args.target = environment(args.target)


def build(args):
    if args.no_bundle:
        print(f'Executable: {cargo_build(args)}')
        return
    config = {'build': {'beforeBundleCommand': None}, 'bundle': {'active': True}}
    # As a file: npx.cmd passes its arguments through cmd.exe, which
    # mangles JSON's quotes.
    config_path = target_dir() / 'tauri-windows-config.json'
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(config), encoding='utf-8')
    command = [*tauri_cli(), 'build', '--bundles', 'nsis', '--ci', '--config', str(config_path)]
    if args.debug:
        command.append('--debug')
    command += ['--target', args.target]
    run(command + ['--', *cargo_args(args)], cwd=DESKTOP)
    base = output_dir(args) / 'bundle/nsis'
    installers = sorted(base.glob('*-setup.exe'))
    if not installers:
        raise SystemExit(f'No Windows installer found in {base}')
    for installer in installers:
        print(f'Windows installer (unsigned): {installer}')


def environment(target):
    """Imports Visual Studio's C++ environment for the target; returns the target."""
    target = target or 'x86_64-pc-windows-msvc'
    architectures = {'x86_64-pc-windows-msvc': 'x64', 'aarch64-pc-windows-msvc': 'arm64'}
    if target not in architectures:
        raise SystemExit(f'Unsupported Windows target: {target}')
    arch = architectures[target]
    component = 'ARM64' if arch == 'arm64' else 'x86.x64'
    vswhere = Path(os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')) / 'Microsoft Visual Studio/Installer/vswhere.exe'
    if not vswhere.exists():
        raise SystemExit('Install Visual Studio C++ Build Tools and the Windows SDK first.')
    result = subprocess.run([str(vswhere), '-latest', '-products', '*', '-requires',
                             f'Microsoft.VisualStudio.Component.VC.Tools.{component}',
                             '-property', 'installationPath'], capture_output=True, text=True, check=True)
    if not result.stdout.strip():
        raise SystemExit(f'No Visual Studio installation with {arch} C++ tools was found.')
    devcmd = Path(result.stdout.strip()) / 'Common7/Tools/VsDevCmd.bat'
    host = 'arm64' if os.environ.get('PROCESSOR_ARCHITECTURE', '').upper() == 'ARM64' else 'x64'
    host_target = next(triple for triple, name in architectures.items() if name == host)
    with tempfile.TemporaryDirectory(prefix='copool-vs-') as directory:
        script = Path(directory) / 'environment.cmd'
        # VsDevCmd's messages go to stderr, kept apart from the variables
        # read from stdout, and shown when it fails.
        script.write_text(f'@echo off\ncall "{devcmd}" -arch={arch} -host_arch={host} 1>&2\n'
                          'if errorlevel 1 exit /b 1\nset\n')
        result = subprocess.run([os.environ.get('COMSPEC', 'cmd.exe'), '/d', '/c', str(script)],
                                capture_output=True, text=True)
        if result.returncode:
            raise SystemExit(f'{devcmd} -arch={arch} -host_arch={host} failed:\n{result.stderr.strip()}')
    for line in result.stdout.splitlines():
        key, separator, value = line.partition('=')
        if key and separator:
            os.environ[key] = value
    cargo_home = Path(os.environ.get('CARGO_HOME') or Path.home() / '.cargo')
    os.environ['PATH'] = str(cargo_home / 'bin') + os.pathsep + os.environ['PATH']
    # The toolchain is the host's, which runs here; a cross build adds the
    # target's standard library to it and passes `--target`.
    toolchain = f'stable-{host_target}'
    os.environ['RUSTUP_TOOLCHAIN'] = toolchain
    try:
        missing = subprocess.run(['rustc', '--version']).returncode != 0
    except FileNotFoundError:
        missing = True
    if missing:
        raise SystemExit(f'Install Rust with: rustup toolchain install {toolchain} --profile minimal')
    if target != host_target:
        installed = subprocess.run(['rustup', 'target', 'list', '--installed'],
                                   capture_output=True, text=True)
        if installed.returncode or target not in installed.stdout.split():
            raise SystemExit(f'Add the target with: rustup target add {target} --toolchain {toolchain}')
    return target
