#!/usr/bin/env python3
"""Verify only retained legacy-read logs; no native launch or clipboard access."""
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
CACHE = Path.home() / '.cache/herdr-browser-prototype/clipboard-transport-proof'
RUN = CACHE / 'legacy-evidence-1794810611108875168'
log = (RUN / 'run/app.log').read_text()
rows = [json.loads(line.removeprefix('LEGACY_CASE ')) for line in log.splitlines() if line.startswith('LEGACY_CASE ')]
assert len(rows) == 11
assert all(row['pass'] is True for row in rows)
assert 'javascript_access_clipboard=DISABLED javascript_dom_paste=DISABLED CLIPBOARD_READ_WRITE=BLOCK permission_prompt=DENY' in log
assert log.count('LEGACY_SEED exact_readback=true') == 3
# The raw fixed test seed must never be printed in the runtime log.
assert 'OWNED_LEGACY_READ_SECRET' not in log
labels = {'A-focused-nonactivated': (True, False), 'A-focused-click-activated': (True, True), 'B-independent-background': (False, False)}
for label, (focus, activation) in labels.items():
    subset = [row for row in rows if row['label'] == label]
    assert len(subset) == 3
    assert {row['operation'] for row in subset} == {'execCommand_paste', 'readText', 'read'}
    for row in subset:
        before, after = row['before'], row['after']
        assert before['focus'] is focus and before['activation'] is activation
        assert after['focus'] is focus and after['activation'] is activation
        assert before['selection'] == [0, 9] and before['editableFocused'] is True
        assert after['valueUnchanged'] is True and after['valueEmpty'] is False
        assert after['domSawSeed'] is False and after['valueExactSeed'] is False
        events = [json.loads(line) for line in after['events'].splitlines()]
        assert not any(event.get('sawSeed') or event.get('dataExactSeed') for event in events)
        if row['operation'] == 'execCommand_paste':
            assert after['legacyResult'] is False and after['legacyThrew'] is False
            assert events == []
        else:
            event = next(event for event in events if event.get('action') == row['operation'])
            assert event['type'] == 'api_read' and event['result'] == 'DENIED'
for mode in ['default', 'cancel']:
    row = next(row for row in rows if row['label'] == mode)
    assert row['operation'] == 'browser_native_Frame_Paste'
    assert row['before']['focus'] is True and row['before']['editableFocused'] is True
    after = row['after']
    events = [json.loads(line) for line in after['events'].splitlines()]
    paste = next(event for event in events if event['type'] == 'paste')
    assert paste['trusted'] is True and paste['dataExactSeed'] is True
    assert paste['dataLength'] == 36 and paste['dataEmpty'] is False
    assert paste['prevented'] is (mode == 'cancel')
    if mode == 'default':
        assert after['valueExactSeed'] is True
        assert {event['type'] for event in events} == {'paste', 'beforeinput', 'input'}
        assert all(event['trusted'] for event in events)
    else:
        assert after['valueUnchanged'] is True
        assert len(events) == 1
assert 'PROOF cleanup all_on_before_close=true' in log
assert 'PROOF cef_shutdown=true' in log
assert json.loads((RUN / 'run/result.json').read_text())['app_exit_code'] == 0
assert '4 passed; 0 failed' in (RUN / 'arguments-tests.log').read_text()
previous = json.loads((HERE / 'summary.json').read_text())
for name in ['probe.rs', 'scenarios.rs', 'arguments.rs']:
    assert hashlib.sha256((HERE / name).read_bytes()).hexdigest() == previous['source_hashes'][name]
workspaces = [json.loads(path.read_text())['workspace'] for path in CACHE.glob('*/run/result.json')]
remaining = []
for proc in Path('/proc').iterdir():
    if not proc.name.isdecimal():
        continue
    try:
        exe = str((proc / 'exe').resolve(strict=True))
        argv = (proc / 'cmdline').read_bytes().split(b'\0')
        if exe.startswith(str(CACHE) + '/') or any(any(workspace.encode() in arg for workspace in workspaces) for arg in argv):
            remaining.append(int(proc.name))
    except OSError:
        pass
assert not remaining, remaining
print(json.dumps({'legacy_read_isolation': {
    'run': str(RUN),
    'settings': {'javascript_access_clipboard': 'DISABLED', 'javascript_dom_paste': 'DISABLED', 'CLIPBOARD_READ_WRITE': 'BLOCK', 'permission_prompt_handler': 'DENY'},
    'attempted': 11, 'passed': 11, 'failed': 0,
    'legacy_execCommand_paste_false_unchanged_no_event': 3,
    'navigator_readText_denied': 3, 'navigator_read_denied': 3,
    'conditions': list(labels),
    'browser_native_paste_exact_trusted_default': True,
    'browser_native_paste_exact_trusted_canceled_unchanged': True,
    'native_paste_data_empty': False,
    'native_paste_data_utf16_length': 36,
    'raw_seed_logged': False,
    'clean_close_and_shutdown': True,
    'remaining_owned_processes_snapshot': remaining,
    'helper_classifier_tests_passed': 4,
    'previous_summary_and_Rust_behavior_unchanged': True,
    'previous_renderer_paste_failures': 20,
    'previous_renderer_paste_empty_cause': 'UNDETERMINED; no policy-fix inference',
    'receipt_scope': 'stock CEF 156 owned-fixture semantics and tested unattended READ gate only; not production acceptance or proof of all read paths',
    'production_accepted': False, 'OS_clipboard_claim': False,
    'source_hashes': {name: hashlib.sha256((HERE / name).read_bytes()).hexdigest() for name in ['legacy_probe.rs', 'legacy_scenarios.rs', 'legacy-fixture.js', 'run-legacy.sh']},
}}, indent=2))
