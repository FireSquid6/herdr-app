#!/usr/bin/env python3
"""Cache-only adapter/build/native run; originals stay byte-for-byte untouched."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

HERE = Path(__file__).resolve().parent
P = Path.home() / '.cache/herdr-browser-prototype'
RUN = P / 'clipboard-transport-proof' / f'cdp-evidence-{time.time_ns()}'
RUN.mkdir(mode=0o700, parents=True)
print(RUN, flush=True)
original = HERE / 'legacy_probe.rs'
text = original.read_text()
def replace(old, new):
    global text
    assert text.count(old) == 1, old
    text = text.replace(old, new)
replace('mod arguments;', f'#[path = {json.dumps(str(HERE / "arguments.rs"))}] mod arguments;')
replace('#[path = "legacy_scenarios.rs"]\nmod scenarios;', f'#[path = {json.dumps(str(HERE / "cdp_paste_probe.rs"))}] mod scenarios;')
replace('const FUNCTION: &str = include_str!("legacy-fixture.js");', 'const FUNCTION: &str = scenarios::FIXTURE;')
# This diagnostic mode belongs only to the new owned fixture adapter.
replace('""|"default"|"custom"|"cancel"', '""|"default"|"custom"|"cancel"|"block-keydown"')
replace('let _=stream.read(&mut request);', 'let _=stream.read(&mut request);scenarios::report(&request);')
# Only this NEW owned fixture has same-origin start/end/paste HTTP proof reports.
# No navigation expansion, network refetch, website/CDP response-policy override.
old = 'if request.is_some_and(|r|CefString::from(&r.url()).to_string()==self.url){ReturnValue::CONTINUE}'
replace(old, 'if request.is_some_and(|r|scenarios::allowed_resource(&CefString::from(&r.url()).to_string(),&self.url)){ReturnValue::CONTINUE}')
replace("frame-src 'none'; connect-src 'none'", "frame-src 'none'; connect-src 'self'")
(RUN / 'probe.rs').write_text(text)
(RUN / 'source-hashes.json').write_text(json.dumps({str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in [original, HERE / 'arguments.rs', HERE / 'cdp_paste_probe.rs', Path(__file__).resolve()]}, indent=2))
cmd = ['rustc', '--edition=2024', str(RUN / 'probe.rs'), '-L', f'dependency={P}/target-app-mvp/debug/deps', '-L', f'native={P}/runtime-156.0.26', '--extern', f'cef={P}/target-app-mvp/debug/deps/libcef-ad1108621cfb5a08.rlib', '--extern', f'serde_json={P}/target-app-mvp/debug/deps/libserde_json-9f3941d56b23a919.rlib', '-C', 'debuginfo=0', '-o', str(RUN / 'cdp-paste-proof')]
(RUN / 'build-command.json').write_text(json.dumps(cmd, indent=2))
with (RUN / 'build.log').open('w') as log:
    subprocess.run(['bash', '-c', 'source /home/firesquid/.local/share/herdr-fork-tools/env.sh; exec "$@"', 'bash', *cmd], stdout=log, stderr=subprocess.STDOUT, check=True)
env = os.environ.copy()
env.update(LD_LIBRARY_PATH=str(P / 'execution-stage-156.0.26'), HERDR_BROWSER_RUNTIME=str(P / 'execution-stage-156.0.26'), CHROME_DEVEL_SANDBOX='/opt/google/chrome/chrome-sandbox', VK_DRIVER_FILES='/usr/share/vulkan/icd.d/lvp_icd.json', NATIVE_WAYLAND_HEADLESS='1')
run_cmd = ['python3', '/home/firesquid/worktrees/herdr-forks/browser-tabs-and-clipboard/prototypes/native-wayland-browser/run-headless.py', '--output', str(RUN / 'run'), '--timeout', '60', '--', str(RUN / 'cdp-paste-proof')]
(RUN / 'run-command.json').write_text(json.dumps(run_cmd, indent=2))
code = subprocess.run(run_cmd, env=env).returncode
log = (RUN / 'run/app.log').read_text()
rows = [json.loads(line.removeprefix('CDP_CASE ')) for line in log.splitlines() if line.startswith('CDP_CASE ')]
gates = [json.loads(line.removeprefix('CDP_GATE ')) for line in log.splitlines() if line.startswith('CDP_GATE ')]
controls = [json.loads(line.removeprefix('CDP_CONTROL ')) for line in log.splitlines() if line.startswith('CDP_CONTROL ')]
keyboard_policy = [json.loads(line.removeprefix('CDP_KEYDOWN_POLICY ')) for line in log.splitlines() if line.startswith('CDP_KEYDOWN_POLICY ')]
summary = {'cases': rows, 'read_gates': gates, 'controls': controls, 'keyboard_policy': keyboard_policy, 'exit_code': code, 'clean_close': 'PROOF cleanup all_on_before_close=true' in log, 'cef_shutdown': 'PROOF cef_shutdown=true' in log, 'formal_barrier_claim': False, 'production_accepted': False, 'OS_clipboard_claim': False}
# Scoped post-run process snapshot, no signals or global process-name matching.
workspace = json.loads((RUN / 'run/result.json').read_text())['workspace']
remaining = []
for proc in Path('/proc').iterdir():
    if not proc.name.isdecimal(): continue
    try:
        exe = str((proc / 'exe').resolve(strict=True))
        args = (proc / 'cmdline').read_bytes().split(b'\0')
        if exe.startswith(str(RUN) + '/') or any(workspace.encode() in arg for arg in args): remaining.append(int(proc.name))
    except OSError: pass
summary['remaining_owned_processes_snapshot'] = remaining
errors = []
expected = {(candidate, mode) for candidate in ('commands', 'ctrl-v', 'native-keyup')
            for mode in ('normal', 'cancel', 'empty', 'blocked')}
if len(rows) != len(expected) or {(r['candidate'], r['mode']) for r in rows} != expected:
    errors.append('incomplete native case matrix')
for row in rows:
    label = row['candidate'] + '/' + row['mode']
    if not all(row[key] for key in ('method_success', 'trusted_paste_exact', 'field_exact', 'keyup_success')):
        errors.append(label + ': native result mismatch')
    events = [json.loads(line) for line in row['source']['events'].splitlines()]
    types = [event['type'] for event in events]
    if row['candidate'] == 'native-keyup' and not ('paste' in types and 'keyup' in types and types.index('paste') < types.index('keyup')):
        errors.append(label + ': keyup did not follow Paste')
    if row['mode'] == 'blocked' and not ('paste' in types and 'block_end' in types and types.index('block_end') < types.index('paste')):
        errors.append(label + ': missing pre-execution block sensitivity')
if len(keyboard_policy) != 3:
    errors.append('incomplete keyboard-cancellation matrix')
for row in keyboard_policy:
    events = [json.loads(line) for line in row['source']['events'].splitlines()]
    pastes = [event for event in events if event['type'] == 'paste']
    native = row['candidate'] == 'native-keyup'
    expected_value = 'OWNED_KEYDOWN_POLICY_SEED' if native else 'UNCHANGED'
    if not row['method_success'] or not row['keyup_success'] or row['source']['value'] != expected_value:
        errors.append(row['candidate'] + ': keyboard-policy mismatch')
    if bool(pastes) != native:
        errors.append(row['candidate'] + ': unexpected keyboard-cancellation Paste behavior')
if len(gates) != 3 or not all(row['both_denied'] for row in gates):
    errors.append('pre-consent read denial failed')
if len(controls) != 1 or not controls[0]['self_interference_reproduced']:
    errors.append('browser-return-only race sensitivity control failed')
if code or not summary['clean_close'] or not summary['cef_shutdown'] or remaining:
    errors.append('native run or owned-process cleanup failed')
summary['validation_errors'] = errors
summary['validated_scoped_cases'] = not errors
(RUN / 'summary.json').write_text(json.dumps(summary, indent=2, ensure_ascii=False))
print(f'Receipt: {RUN / "summary.json"}', flush=True)
raise SystemExit(1 if errors else 0)
