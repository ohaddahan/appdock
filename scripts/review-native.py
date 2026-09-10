#!/usr/bin/env python3
"""Serial, disposable-only native validation. Never launch historical user-app smoke modes."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--binary', default='target/debug/appdock')
parser.add_argument('--output', required=True)
parser.add_argument('--cases', default='A1,A2,A3,A4,A5,A6,D3,D4,D5,D6,movement,restoration,frame,pointer,rename,disconnected,tracking,Startup,Minimized,Animations,RestoreReady')
args = parser.parse_args()
binary = Path(args.binary).resolve()
out = Path(args.output).resolve()
out.mkdir(parents=True, exist_ok=True)
smokes = {'movement': '--movement-fixture', 'restoration': '--restoration-fixture', 'frame': '--frame-smoke', 'pointer': '--pointer-smoke', 'rename': '--rename-smoke', 'disconnected': '--disconnected-smoke', 'tracking': '--tracking-smoke', 'design': '--design-smoke', 'surface': '--surface-smoke', 'settings': '--ui-smoke'}
review = {'prerequisite', 'A1', 'A2', 'A3', 'A4', 'A5', 'A6', 'D3', 'D4', 'D5', 'D6', 'Minimized', 'Startup', 'Animations', 'RestoreReady'}
cases = args.cases.split(',')
if any(case not in review and case not in smokes for case in cases):
    parser.error('Only disposable review cases and the allowlisted smoke modes are permitted')

def command_output(command):
    return subprocess.check_output(command, text=True).strip()

report = {'started_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'environment': {'platform': platform.platform(), 'machine': platform.machine(),
                          'macos': command_output(['sw_vers']), 'rust': command_output(['rustc', '--version']),
                          'baseline_commit': command_output(['git', 'rev-parse', 'HEAD']), 'binary': str(binary),
                          'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()},
          'cases': []}

def run(case):
    command = [str(binary), smokes[case]] if case in smokes else [str(binary), '--review-fixture', case]
    started = time.monotonic()
    log = out / (case + '.log')
    with tempfile.TemporaryDirectory(prefix='appdock-review-') as data:
        env = dict(os.environ, APPDOCK_DATA_DIR=data, APPDOCK_SMOKE_SCREENSHOT=str(out / (case + '.png')))
        if case == 'settings':
            env.update(APPDOCK_SETTINGS_SMOKE='1', APPDOCK_SMOKE_TICKS='20')
        with log.open('w') as handle:
            process = subprocess.Popen(command, stdout=handle, stderr=subprocess.STDOUT, env=env, start_new_session=True)
            try:
                code = process.wait(timeout=180)
            except subprocess.TimeoutExpired:
                code = 124
            finally:
                # This group contains only this runner's case and its disposable children.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
    status = 'passed' if code == 0 else 'prerequisite failure' if code == 2 else 'failed'
    entry = {'id': case, 'outcome': status, 'exit_code': code, 'duration_seconds': round(time.monotonic()-started, 3), 'log': log.name}
    report['cases'].append(entry)
    (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(entry), flush=True)
    return code

prerequisite = run('prerequisite')
if prerequisite:
    for case in cases:
        report['cases'].append({'id': case, 'outcome': 'prerequisite failure', 'reason': 'Accessibility prerequisite failed; case not run'})
    (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    raise SystemExit(prerequisite)
failed = False
for case in cases:
    failed |= run(case) != 0
raise SystemExit(1 if failed else 0)
