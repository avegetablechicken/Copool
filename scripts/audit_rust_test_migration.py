#!/usr/bin/env python3
"""Inventory original Swift tests and report executable ports without hiding gaps.

--require-complete fails while any original case is unported. This is distinct
from running the currently implemented Rust/frontend tests successfully.
"""
import argparse
import json
import re
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def inventory():
    cases = {}
    for path in sorted((ROOT/'Tests/CopoolTests').glob('*.swift')):
        source = path.read_text()
        names = re.findall(r'\bfunc (test\w+)\s*\(',source)
        names += re.findall(r'@Test(?:\([^\n]*\))?\s+func (\w+)\s*\(',source)
        for name in names:
            cases[f'{path.stem}.{name}'] = {'source':str(path.relative_to(ROOT)), 'status':'pending', 'target':None}
    return cases

def audit():
    cases = inventory()
    for path in sorted((ROOT/'crates/copool-core/src/legacy_tests').glob('*.rs')):
        for name in re.findall(r'#\[(?:tokio::)?test[^\]]*\]\s*(?:async\s+)?fn\s+(\w+)\s*\(',path.read_text()):
            key=f'{path.stem}.{name}'
            if key not in cases: raise ValueError(f'Unknown Swift test ID: {key}')
            cases[key].update(status='ported',target=str(path.relative_to(ROOT)))
    proxy_tests=ROOT/'Sources/Copool/Resources/proxyd-src/src/legacy_proxy_tests.rs'
    if proxy_tests.exists():
        for name in re.findall(r'\bfn (test\w+)\s*\(',proxy_tests.read_text()):
            key=f'SwiftNativeProxyRuntimeServiceTests.{name}'
            if key not in cases: raise ValueError(f'Unknown Swift test ID: {key}')
            cases[key].update(status='ported',target=str(proxy_tests.relative_to(ROOT)))
    for path in sorted((ROOT/'desktop/tests').glob('*.test.cjs')):
        source=path.read_text()
        for suite,name in re.findall(r'''legacy\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"]''',source):
            key=f'{suite}.{name}'
            if key not in cases: raise ValueError(f'Unknown Swift test ID: {key}')
            cases[key].update(status='ported',target=str(path.relative_to(ROOT)))
        # Parameterized layout scenarios retain their original test names.
        if 'presentation.test.cjs' == path.name:
            for name in ['testIPadMiniPortraitColumnCounts','testIPadMiniLandscapeColumnCounts','testRegularIPadPortraitColumnCounts','testRegularIPadLandscapeColumnCounts']:
                if name in source: cases[f'LayoutRulesTests.{name}'].update(status='ported',target=str(path.relative_to(ROOT)))
    return cases

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--require-complete',action='store_true')
    parser.add_argument('--output',type=Path)
    args=parser.parse_args()
    cases=audit(); ported=sum(c['status']=='ported' for c in cases.values())
    print(f'Swift test migration: {ported}/{len(cases)} ported; {len(cases)-ported} pending')
    if args.output:
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps({'total':len(cases),'ported':ported,'pending':len(cases)-ported,'cases':cases},indent=2)+'\n')
    return int(args.require_complete and ported != len(cases))
if __name__=='__main__':raise SystemExit(main())
