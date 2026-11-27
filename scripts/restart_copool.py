#!/usr/bin/env python3
"""Restart Copool: stop the copies running from one place, then start it there again.

Only processes whose executable lies in that place are stopped, so a copy
installed elsewhere is left running (and, holding Copool's single instance,
would then keep the new start from staying up, which is reported).

  macOS     --app path/to/Copool.app
  Windows   --app path\\to\\Copool.exe, or the directory it is installed in
  Linux     --app path/to/copool-desktop, started through linux_build.py's
            command prefix; without --app, the installation linux_build.py
            names, if any

The processes are asked to quit (SIGTERM; taskkill without /F) and, if they
are still running after --timeout seconds, killed. Copool starts only after
they are gone, as its data directory admits one instance at a time.
"""
import argparse
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

MACOS = sys.platform == 'darwin'
WINDOWS = os.name == 'nt'


def target(app):
    """(where the executables to stop lie, the command that starts Copool)."""
    if app is None:
        if not MACOS and not WINDOWS:
            import linux_build
            installed = linux_build.installation()
            if installed:
                return installed
        raise SystemExit('Name the copy to restart with --app.')
    app = Path(app).expanduser().resolve()
    if MACOS:
        if app.suffix != '.app' or not (app / 'Contents/MacOS').is_dir():
            raise SystemExit(f'Not an application bundle: {app}')
        return app / 'Contents/MacOS', ['open', str(app)]
    if WINDOWS:
        executable = app / 'Copool.exe' if app.is_dir() else app
        if not executable.is_file():
            raise SystemExit(f'No executable at {executable}')
        return executable.parent, [str(executable)]
    if not app.is_file() or not os.access(app, os.X_OK):
        raise SystemExit(f'Not an executable: {app}')
    import linux_build
    return app, [*linux_build.command_prefix(), str(app)]


def processes():
    """(pid, executable path) of the processes this user can see."""
    if WINDOWS:
        script = ('Get-CimInstance Win32_Process | Where-Object ExecutablePath | '
                  'ForEach-Object { "$($_.ProcessId)`t$($_.ExecutablePath)" }')
        output = subprocess.run(['powershell', '-NoProfile', '-Command', script],
                                capture_output=True, text=True, check=True).stdout
        rows = (line.split('\t', 1) for line in output.splitlines() if '\t' in line)
        return [(int(pid), path) for pid, path in rows]
    if MACOS:
        output = subprocess.run(['ps', '-axww', '-o', 'pid=,comm='],
                                capture_output=True, text=True, check=True).stdout
        rows = (line.strip().split(None, 1) for line in output.splitlines())
        return [(int(row[0]), row[1]) for row in rows if len(row) == 2]
    found = []
    for entry in Path('/proc').iterdir():
        if entry.name.isdigit():
            try:
                path = os.readlink(entry / 'exe')
            except OSError:
                continue
            # A binary replaced while it runs, as a rebuild does.
            found.append((int(entry.name), path[:-len(' (deleted)')] if path.endswith(' (deleted)') else path))
    return found


def running(scope):
    def inside(path):
        path = Path(path)
        return path == scope or scope in path.parents
    return sorted(pid for pid, path in processes() if pid != os.getpid() and inside(path))


def stop(pids, force):
    for pid in pids:
        if WINDOWS:
            subprocess.run(['taskkill', *(['/F'] if force else []), '/PID', str(pid)], capture_output=True)
            continue
        try:
            os.kill(pid, signal.SIGKILL if force else signal.SIGTERM)
        except ProcessLookupError:
            pass


def wait(scope, until, timeout):
    """Waits until `until(pids)` holds for the processes in scope; returns them."""
    deadline = time.monotonic() + timeout
    while True:
        pids = running(scope)
        if until(pids) or time.monotonic() >= deadline:
            return pids
        time.sleep(0.2)


def start(command):
    print('+ ' + ' '.join(command), flush=True)
    quiet = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if MACOS:
        subprocess.run(command, check=True)
    elif WINDOWS:
        flags = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP
        subprocess.Popen(command, creationflags=flags, close_fds=True, **quiet)
    else:
        subprocess.Popen(command, start_new_session=True, cwd=Path.home(), **quiet)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--app', help='the copy to restart (see above)')
    parser.add_argument('--timeout', type=float, default=15, help='seconds to wait for each step (default 15)')
    parser.add_argument('--dry-run', action='store_true', help='show what would be stopped and started')
    args = parser.parse_args()
    scope, command = target(args.app)
    pids = running(scope)
    print(f'Running from {scope}: {", ".join(map(str, pids)) or "none"}', flush=True)
    if args.dry_run:
        print('Would start: ' + ' '.join(command))
        return
    if pids:
        stop(pids, force=False)
        left = wait(scope, lambda pids: not pids, args.timeout)
        if left:
            print(f'Still running after {args.timeout:g}s, killing: {", ".join(map(str, left))}', flush=True)
            stop(left, force=True)
            if wait(scope, lambda pids: not pids, 5):
                raise SystemExit('Copool did not stop; not starting another.')
    start(command)
    started = wait(scope, bool, args.timeout)
    if not started:
        raise SystemExit(f'Copool is not running from {scope} after {args.timeout:g}s; '
                         'a copy running elsewhere may have kept it from starting.')
    # A start that hands over to another instance exits soon after.
    time.sleep(2)
    started = running(scope)
    if not started:
        raise SystemExit(f'Copool started from {scope} but exited; '
                         'a copy running elsewhere may hold its single instance.')
    print(f'Copool restarted from {scope}: {", ".join(map(str, started))}')


if __name__ == '__main__':
    main()
