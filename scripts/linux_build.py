"""Linux steps of the shared desktop build: the executable, checked for its libraries.

The build only builds. A machine whose GUI libraries need their own
environment, or that installs builds for restart_copool.py to restart,
changes only the hooks below: `command_prefix`, `installation` and `install`,
with any options of its own in `add_arguments`.
"""
import subprocess

from build_common import cargo_build


def add_arguments(parser):
    pass


def prepare(args):
    pass


def command_prefix():
    """The command that Cargo and the library check run through, e.g. a wrapper
    that sets up a GUI SDK's environment; restart_copool.py starts a binary
    through it too."""
    return []


def installation():
    """The installed copy that restart_copool.py restarts by default, as
    (directory its executables run from, command that starts it); None when
    nothing is installed."""
    return None


def install(binary):
    """Installs a build as the copy `installation` names, for
    `restart_copool.py --install`; a machine that installs nothing refuses."""
    raise SystemExit('This machine installs no builds: linux_build.install is not set up.')


def check_libraries(binary, prefix):
    """Every shared library resolves, in the environment the binary runs in."""
    linked = subprocess.run([*prefix, 'ldd', str(binary)], text=True, capture_output=True)
    output = linked.stdout + linked.stderr
    if linked.returncode or 'not found' in output:
        raise SystemExit(f'Missing runtime libraries of {binary}:\n{output}')


def build(args):
    prefix = command_prefix()
    binary = cargo_build(args, prefix)
    if not args.target:
        check_libraries(binary, prefix)
    print(f'Executable: {binary}')
