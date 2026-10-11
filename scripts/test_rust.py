#!/usr/bin/env python3
"""Cross-platform test entry point; no Xcode, live accounts or external API keys."""
import argparse
import subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--offline',action='store_true')
args=parser.parse_args()
commands=[['cargo','fmt','--all','--','--check'],['cargo','test','-p','copool-core','--locked']+(['--offline'] if args.offline else []),['node','--test',*map(str,sorted((ROOT/'desktop/tests').glob('*.test.cjs')))],['cargo','check','--manifest-path','resources/proxyd-src/proxyd/Cargo.toml','--locked']+(['--offline'] if args.offline else [])]
commands[2:2]=[['node','--check',str(path)] for path in sorted((ROOT/'desktop/ui').glob('*.js'))]
status=0
for command in commands:
    print('+ '+' '.join(command),flush=True)
    result=subprocess.run(command,cwd=ROOT)
    if result.returncode:status=1
raise SystemExit(status)
