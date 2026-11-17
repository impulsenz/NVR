#!/usr/bin/env python3
"""Benchmark the running GAZE test server with N simulated cameras.
Usage: bench.py N [viewers] [label]
Sets up N cameras (main 2560x1440 + sub 640x360), waits, then samples for
30 s: CPU of GAZE and all its child processes, their memory (RSS), and the
number of ffmpeg processes. `viewers` headless browsers keep the Live page
open meanwhile."""
import json, os, subprocess, sys, time, http.cookiejar, urllib.request
S = os.path.dirname(os.path.abspath(__file__))
B = os.environ['BENCH']  # scratch folder (see README)
N = int(sys.argv[1]); V = int(sys.argv[2]) if len(sys.argv) > 2 else 0
LABEL = sys.argv[3] if len(sys.argv) > 3 else ''
BASE = 'http://localhost:18080/'
cj = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj))
def api(path, method='GET', body=None):
    r = urllib.request.Request(BASE + path, method=method, data=json.dumps(body).encode() if body is not None else None,
                               headers={'Content-Type': 'application/json', 'Origin': 'http://localhost:18080'})
    with op.open(r) as f: return json.loads(f.read() or b'null')
api('api/auth/login', 'POST', {'username': 'admin', 'password': 'testpass123'})
subprocess.run([S + '/cams.sh', str(N)])
cams = api('api/cameras')
for c in cams[N:]: api('api/cameras/' + c['id'], 'DELETE')
for i in range(len(cams) + 1, N + 1):
    api('api/cameras', 'POST', {'name': f'Cam {i}', 'url': f'rtsp://127.0.0.1:8554/cam{i}', 'live_url': f'rtsp://127.0.0.1:8554/cam{i}sub', 'record_mode': 'continuous'})
nvr = int(subprocess.check_output(['pgrep', '-f', '/release/nvr ' + B]).split()[0])
viewers = [subprocess.Popen(['node', S + '/viewer.js', '75']) for _ in range(V)]
time.sleep(30)
def tree(pid):
    out = {pid}
    ch = subprocess.run(['pgrep', '-P', str(pid)], capture_output=True, text=True).stdout.split()
    for c in ch: out |= tree(int(c))
    return out
def snap():
    tot, pss, ff = 0, 0, 0
    for p in tree(nvr):
        try:
            st = open(f'/proc/{p}/stat').read().rsplit(')', 1)[1].split()
            tot += int(st[11]) + int(st[12])
            # PSS: shared library pages split between the processes using
            # them (plain RSS counts ffmpeg's libraries once per process).
            pss += next(int(l.split()[1]) for l in open(f'/proc/{p}/smaps_rollup') if l.startswith('Pss:')) * 1024
            if 'ffmpeg' in open(f'/proc/{p}/comm').read(): ff += 1
        except (FileNotFoundError, ProcessLookupError, StopIteration): pass
    return tot, pss, ff
t0, _, _ = snap(); w0 = time.time()
samples = []
for _ in range(6):
    time.sleep(5); samples.append(snap())
t1 = samples[-1][0]; dt = time.time() - w0
hz = os.sysconf('SC_CLK_TCK')
cpu = (t1 - t0) / hz / dt * 100
pss = max(s[1] for s in samples) / 1e6
ff = max(s[2] for s in samples)
for v in viewers: v.terminate()
res = {'label': LABEL, 'cameras': N, 'viewers': V, 'cpu_percent_of_one_core': round(cpu, 1), 'pss_mb': round(pss), 'ffmpeg_processes': ff, 'cores': os.cpu_count()}
print(json.dumps(res))
open(B + '/bench-results.jsonl', 'a').write(json.dumps(res) + '\n')
