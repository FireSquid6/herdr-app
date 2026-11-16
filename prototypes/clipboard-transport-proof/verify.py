#!/usr/bin/env python3
"""Read retained owned evidence; no GUI/process signals/clipboard access."""
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
# Invoke from the normal-user environment, not a harness-private HOME.
CACHE = Path.home() / '.cache/herdr-browser-prototype/clipboard-transport-proof'
FINAL = CACHE / 'evidence-1794809931978689706'
ASSERT = CACHE / 'evidence-1794809930522515433'
log = (FINAL / 'run/app.log').read_text()
lines = log.splitlines()
semantic = [line for line in lines if line.startswith('PROOF semantic repeat=')]
assert len(semantic) == 100
counts = {}
trusted_types = set()
for line in semantic:
    operation = line.split('operation=', 1)[1].split()[0]
    mode = line.split('mode=', 1)[1].split()[0]
    counts[f'{operation}/{mode}'] = counts.get(f'{operation}/{mode}', 0) + 1
    source, _ = json.JSONDecoder().raw_decode(line.split(' source=', 1)[1])
    events = [json.loads(event) for event in source['events'].splitlines()]
    for event in events:
        assert event['trusted'] is True
        trusted_types.add(event['type'])
    assert any(event['type'] == operation for event in events)
    if mode == 'cancel':
        assert next(event for event in events if event['type'] == operation)['prevented']
    if operation == 'cut' and mode == 'default':
        assert {event['type'] for event in events} >= {'cut', 'beforeinput', 'input'}
assert all(count == 10 for count in counts.values())
assert 'semantic_complete repeats=10 cases=100 failed=20' in log
assert log.count('old_algorithm_would_export_wrong_workspace=true') == 4
assert log.count('renderer_paste_exact_data=false') == 20
assert log.count('PROOF unattended_read') == 2
assert log.count('export=None') == 40
assert '"type":"write","result":"ALLOWED"' in log.replace('\\"', '"')
assert 'PROOF normal_useractivated_website_write_exact_readback=' in log
assert 'PROOF cleanup all_on_before_close=true' in log
assert 'PROOF cef_shutdown=true' in log
assert 'runtime_missing_contract: 4 wrong-owner counterexamples; 20 semantic failures' in log
assert json.loads((FINAL / 'run/result.json').read_text())['app_exit_code'] == 1
assert_log = (ASSERT / 'run/app.log').read_text()
assert 'deliberate cleanup-proof assertion' in assert_log
assert 'PROOF cleanup all_on_before_close=true' in assert_log
assert 'PROOF cef_shutdown=true' in assert_log
assert 'scenario assertion panic (closed all browsers)' in assert_log
# Scope this post-run snapshot to our executables and retained private workspaces.
workspaces = [json.loads(p.read_text())['workspace'] for p in CACHE.glob('evidence-*/run/result.json')]
remaining = []
for proc in Path('/proc').iterdir():
    if not proc.name.isdecimal():
        continue
    try:
        exe = str((proc / 'exe').resolve(strict=True))
        argv = (proc / 'cmdline').read_bytes().split(b'\0')
        if any(exe.startswith(str(root) + '/') for root in (HERE, CACHE)) or any(any(workspace.encode() in arg for workspace in workspaces) for arg in argv):
            remaining.append(int(proc.name))
    except (OSError, PermissionError):
        pass
assert not remaining, remaining
old_arguments = Path('/home/firesquid/worktrees/herdr-forks/browser-clipboard-app/src/browser/arguments.rs')
assert (HERE / 'arguments.rs').read_bytes() == old_arguments.read_bytes()
summary = {
    'outcome': 'runtime_missing_contract',
    'evidence_root': str(CACHE),
    'final_run': str(FINAL),
    'assertion_cleanup_run': str(ASSERT),
    'semantic_repeats_final': 10,
    'source_operations_final': 100,
    'counts': counts,
    'copy_cut_cases_passed': 80,
    'paste_exact_data_failures': 20,
    'renderer_paste_empty_data_cause': 'UNDETERMINED; no inference that a policy fix is needed',
    'no_write_sentinel_suppressed': 40,
    'wrong_owner_counterexamples': 4,
    'wrong_owner_counterexample_scope': 'mechanism-level controlled B native Copy interleaving; not a demonstrated bypass of a globally serialized app coordinator',
    'combined_canceled_A_handler_website_write_case_run': False,
    'unattended_read_denials': 2,
    'activated_website_write': 'ALLOWED, exact readback; normal behavior, not blocker',
    'trusted_source_event_types': sorted(trusted_types),
    'clean_close_and_shutdown': True,
    'assertion_clean_close_and_shutdown': True,
    'remaining_owned_processes_snapshot': remaining,
    'helper_classifier_identical_to_prior': True,
    'source_hashes': {name: hashlib.sha256((HERE / name).read_bytes()).hexdigest() for name in ['probe.rs', 'scenarios.rs', 'arguments.rs', 'run.sh']},
    'production_barrier': False,
    'OS_clipboard_claim': False,
}
print(json.dumps(summary, indent=2, ensure_ascii=False))
