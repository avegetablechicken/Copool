#!/usr/bin/env python3
"""Inventory original Swift tests and report executable ports without hiding gaps.

--require-complete fails while any original case is unported. This is distinct
from running the currently implemented Rust/frontend tests successfully.
Retired cases covered SwiftUI layout that the Coport-style panel replaced; each
is listed with its reason and never counted as ported.
"""
import argparse
import json
import re
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

PANEL = 'The Coport-style panel replaced this SwiftUI layout'
# Whole suites, or single Suite.testName cases.
RETIRED = {
    'LayoutRulesTests': f'{PANEL}: fixed 542 pt grid and iOS column counts',
    'LiquidProgressMetricsTests.testCompactRingUsesDotThresholdAboveOnePercent': f'{PANEL}: no usage rings',
    'LiquidProgressMetricsTests.testDarkGroovePaletteKeepsRingTrackVisibleOnBlackBackground': f'{PANEL}: no glass groove palette',
    'AccountsActionPresentationTests.testLeadingToolbarButtonsIncludeUsageToggleIcon': f'{PANEL}: usage display is set in Settings only',
}

def inventory():
    cases = {}
    for path in sorted((ROOT/'Tests/CopoolTests').glob('*.swift')):
        source = path.read_text()
        names = re.findall(r'\bfunc (test\w+)\s*\(',source)
        names += re.findall(r'@Test(?:\([^\n]*\))?\s+func (\w+)\s*\(',source)
        for name in names:
            cases[f'{path.stem}.{name}'] = {'source':str(path.relative_to(ROOT)), 'status':'pending', 'target':None}
    return cases

def port(cases, key, path):
    if key not in cases: raise ValueError(f'Unknown Swift test ID: {key}')
    if cases[key]['status'] == 'retired': raise ValueError(f'Retired Swift test is ported: {key}')
    cases[key].update(status='ported',target=str(path.relative_to(ROOT)))

def audit():
    cases = inventory()
    for key, reason in RETIRED.items():
        matched = [k for k in cases if k == key or k.startswith(f'{key}.')]
        if not matched: raise ValueError(f'Unknown retired Swift test: {key}')
        for k in matched: cases[k].update(status='retired',reason=reason)
    for path in sorted((ROOT/'crates/copool-core/src/legacy_tests').glob('*.rs')):
        for name in re.findall(r'#\[(?:tokio::)?test[^\]]*\]\s*(?:async\s+)?fn\s+(\w+)\s*\(',path.read_text()):
            port(cases, f'{path.stem}.{name}', path)
    proxy_tests=ROOT/'Sources/Copool/Resources/proxyd-src/src/legacy_proxy_tests.rs'
    if proxy_tests.exists():
        for name in re.findall(r'\bfn (test\w+)\s*\(',proxy_tests.read_text()):
            port(cases, f'SwiftNativeProxyRuntimeServiceTests.{name}', proxy_tests)
    for path in sorted((ROOT/'desktop/tests').glob('*.test.cjs')):
        for suite,name in re.findall(r'''legacy\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"]''',path.read_text()):
            port(cases, f'{suite}.{name}', path)
    return cases

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--require-complete',action='store_true')
    parser.add_argument('--output',type=Path)
    args=parser.parse_args()
    cases=audit()
    count=lambda status: sum(c['status']==status for c in cases.values())
    ported, retired, pending = count('ported'), count('retired'), count('pending')
    print(f'Swift test migration: {ported}/{len(cases)} ported; {retired} retired; {pending} pending')
    if args.output:
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps({'total':len(cases),'ported':ported,'retired':retired,'pending':pending,'cases':cases},indent=2)+'\n')
    return int(args.require_complete and pending != 0)
if __name__=='__main__':raise SystemExit(main())
