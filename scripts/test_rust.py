#!/usr/bin/env python3
"""Cross-platform test entry point; no Xcode, live accounts or external API keys."""
import argparse
import subprocess
import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--offline',action='store_true')
parser.add_argument('--require-parity',action='store_true',help='Fail until all original Swift cases have executable ports')
args=parser.parse_args()
commands=[['cargo','fmt','--all','--','--check'],['cargo','test','-p','copool-core','--locked']+(['--offline'] if args.offline else []),['node','--test',*map(str,sorted((ROOT/'desktop/tests').glob('*.test.cjs')))],['cargo','check','--manifest-path','Sources/Copool/Resources/proxyd-src/proxyd/Cargo.toml','--locked']+(['--offline'] if args.offline else [])]
commands[2:2]=[['node','--check',str(path)] for path in sorted((ROOT/'desktop/ui').glob('*.js'))]
status=0
for command in commands:
    print('+ '+' '.join(command),flush=True)
    result=subprocess.run(command,cwd=ROOT)
    if result.returncode:status=1
report=[sys.executable,'scripts/audit_rust_test_migration.py','--output','artifacts/tests/swift-migration.json']
if args.require_parity:report.append('--require-complete')
if subprocess.run(report,cwd=ROOT).returncode:status=1
raise SystemExit(status)
