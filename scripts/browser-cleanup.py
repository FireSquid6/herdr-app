#!/usr/bin/env python3
"""Test-only cleanup barrier for browser-integration's private scratch resources.

No ensure, navigation, JS or arbitrary RPC entrypoint. Stops only browser IDs
already stored in this runner's isolated core state, after its own GUI exits.
"""
import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import sys
import time

run = Path(sys.argv[1])
core = Path(sys.argv[2])
root = run / 's'
if (run.parent != Path('/var/tmp') or not run.name.startswith('ha.') or run.is_symlink()
        or run.stat().st_mode & 0o077 or (root / '.sandbox-owner').read_text().strip() != f'{os.getuid()}:browser-mvp'):
    sys.exit('refusing cleanup outside private runner ownership')
app_pid = root / 'pid'
if app_pid.exists():
    pid = int(app_pid.read_text())
    envfile = Path(f'/proc/{pid}/environ')
    if envfile.exists():
        if f'HERDR_APP_HOME={root}/home'.encode() not in envfile.read_bytes().split(b'\0'):
            sys.exit('refusing to signal unrelated app PID')
        fd = os.pidfd_open(pid)
        os.kill(pid, signal.SIGTERM)
        if not select.select([fd], [], [], 10)[0]:
            sys.exit('owned app did not exit; leaving profiles intact')
        os.close(fd)
    app_pid.unlink()

env = os.environ.copy()
for key in ['HC_SOCKET', 'HC_SCOPE', 'HC_TERMINAL_SOCKET', 'HC_TERMINAL_ID']:
    env.pop(key, None)
env.update(HOME=str(run/'home'), HC_SESSION='herdr-app-browser-mvp',
           HERDR_APP_HOME=str(root/'home'), TMPDIR=str(root/'tmp'))
for suffix in ['STATE','CONFIG','CACHE','DATA','RUNTIME']:
    env[f'XDG_{suffix}_HOME' if suffix != 'RUNTIME' else 'XDG_RUNTIME_DIR'] = str(root/suffix.lower())
workspaces = sorted(p.name for p in (root/'state/herdr-core/herdr-app-browser-mvp/browsers').glob('*') if p.is_dir() and re.fullmatch('[A-Za-z0-9_-]{1,128}', p.name))
if not workspaces:
    print(json.dumps({'stopped': [], 'reason':'no browser profiles started'}))
    sys.exit(0)
p = subprocess.Popen([str(core), 'connect'], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
def exact(n):
    result = b''
    deadline = time.monotonic()+20
    while len(result)<n:
        if not select.select([p.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
            raise RuntimeError('cleanup RPC timed out; profiles retained')
        chunk = os.read(p.stdout.fileno(), n-len(result))
        if not chunk: raise RuntimeError('cleanup core disconnected')
        result += chunk
    return result
seq = 0
def call(method, params):
    global seq
    seq += 1
    data=json.dumps({'jsonrpc':'2.0','id':seq,'method':method,'params':params}).encode()
    p.stdin.write(struct.pack('<IBI',0,0,len(data))+data);p.stdin.flush()
    channel,kind,size=struct.unpack('<IBI',exact(9))
    if channel or kind or size>4194304: raise RuntimeError('invalid cleanup reply')
    reply=json.loads(exact(size))
    if 'error' in reply: raise RuntimeError(json.dumps(reply['error']))
    return reply['result']
try:
    p.stdin.write(b'{"jsonrpc":"2.0","id":0,"method":"hello","params":{"framed":true}}\n');p.stdin.flush()
    line=b''
    while not line.endswith(b'\n'):
        line+=exact(1)
        if len(line)>4194304: raise RuntimeError('oversized hello')
    call('browser.negotiate', {'revision':2,'client':'herdr-native'})
    stopped=[]
    for workspace in workspaces:
        status=call('browser.status',{'workspace':workspace})
        if status['state']=='running':
            lease=call('browser.acquire',{'workspace':workspace,'force':True,'confirmed':True})
            result=call('browser.stop',{'workspace':workspace,'generation':lease['generation'],'confirmed':True})
            if result.get('state')!='stopped': raise RuntimeError('Chrome did not stop; profiles retained')
            stopped.append(workspace)
    print(json.dumps({'stopped':stopped,'chrome_exit_observed':True}))
finally:
    p.stdin.close()
    try:p.wait(timeout=5)
    except subprocess.TimeoutExpired:p.terminate();p.wait(timeout=5)
