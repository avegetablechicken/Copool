"""Linux steps of the shared desktop build: the executable, checked for its libraries.

A machine whose GUI libraries need their own environment, or that installs
what it builds, changes only the hooks below: `command_prefix`, `after_build`
and `installation`, with any options of its own in `add_arguments`.
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


def after_build(args, binary, prefix):
    """What follows a successful build, e.g. installing it."""


def installation():
    """The installed copy that restart_copool.py restarts by default, as
    (directory its executables run from, command that starts it); None when
    nothing is installed."""
    return None


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
    after_build(args, binary, prefix)
