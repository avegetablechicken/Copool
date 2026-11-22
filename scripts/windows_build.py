"""Prepare Visual Studio for the shared desktop build entry point."""
import os
from pathlib import Path
import subprocess
import tempfile


def prepare(target):
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
    os.environ['PATH'] = str(Path.home() / '.cargo/bin') + os.pathsep + os.environ['PATH']
    os.environ['RUSTUP_TOOLCHAIN'] = f'stable-{target}'
    if subprocess.run(['rustc', '--version']).returncode:
        raise SystemExit(f'Install Rust with: rustup toolchain install stable-{target} --profile minimal --force-non-host')
    return target
