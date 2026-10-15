"""How local macOS builds are signed, shared by the build scripts.

With an Apple Development certificate in the login keychain, the app and its
widget are signed with it, and share the app group `<Team ID>.io.github.copool.rust`:
macOS lets an app use a group named after its Team ID, so the group needs no
provisioning profile and no consent. Without one, both are signed ad hoc and
keep `group.io.github.copool.rust`, which macOS 15 lets the app use only after
asking, and the sandboxed widget not at all.

`COPOOL_SIGN_IDENTITY` names another identity (its SHA-1 or common name), or
`-` to sign ad hoc.
"""
import os
import re
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NATIVE = ROOT / 'desktop/native'
BUNDLE_ID = 'io.github.copool.rust'
AD_HOC_GROUP = f'group.{BUNDLE_ID}'
# The Info.plist key both executables read their app group from.
APP_GROUP_KEY = 'CopoolAppGroup'


def _output(*command):
    return subprocess.run(command, capture_output=True, text=True, check=True).stdout


def identity():
    """(SHA-1, Team ID) of the identity to sign with; ('-', None) for ad hoc."""
    wanted = os.environ.get('COPOOL_SIGN_IDENTITY', '').strip()
    if wanted == '-':
        return '-', None
    identities = re.findall(r'\d+\) ([0-9A-F]{40}) "([^"]+)"',
                            _output('security', 'find-identity', '-v', '-p', 'codesigning'))
    for sha1, name in identities:
        if (sha1 == wanted.upper() or name == wanted) if wanted else name.startswith('Apple Development:'):
            return sha1, _team(sha1, name)
    if wanted:
        raise SystemExit(f'Signing identity not found: {wanted}')
    return '-', None


def _team(sha1, name):
    """The Team ID: the organizational unit of the identity's certificate."""
    pems = _output('security', 'find-certificate', '-a', '-Z', '-p', '-c', name)
    for block in pems.split('SHA-1 hash: ')[1:]:
        if block.startswith(sha1):
            pem = block[block.index('-----BEGIN'):]
            subject = subprocess.run(['openssl', 'x509', '-noout', '-subject', '-nameopt', 'multiline'],
                                     input=pem, capture_output=True, text=True, check=True).stdout
            match = re.search(r'organizationalUnitName\s*=\s*(\w+)', subject)
            if match:
                return match.group(1)
    raise SystemExit(f'No Team ID in the certificate of {name}')


def app_group(team):
    return f'{team}.{BUNDLE_ID}' if team else AD_HOC_GROUP


def entitlements(name, group):
    """`desktop/native/{name}.entitlements` with the build's app group."""
    text = (NATIVE / f'{name}.entitlements').read_text().replace(AD_HOC_GROUP, group)
    handle, path = tempfile.mkstemp(prefix=f'copool-{name}-', suffix='.entitlements')
    with os.fdopen(handle, 'w') as stream:
        stream.write(text)
    return path


def sign(bundle, name, sha1, group, run=subprocess.run):
    """Signs a bundle with the identity and its entitlements for the group."""
    path = entitlements(name, group)
    try:
        result = run(['codesign', '--force', '--sign', sha1, '--entitlements', path, str(bundle)])
        if getattr(result, 'returncode', 0):
            raise SystemExit(result.returncode)
    finally:
        os.unlink(path)
