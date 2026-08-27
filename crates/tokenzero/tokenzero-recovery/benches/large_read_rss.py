#!/usr/bin/env python3
"""Measure release large-read RSS and wall time with deterministic corpora.

A full UTF-8 payload is required for rendering and the response, but a second
recovery payload is not required for stable files: path plus exact line range
is sufficient. Four concurrent subprocesses are reported with a conservative
ceiling: the sum of their independently observed peaks (peaks may not align).
The harness owns the machine-wide heavy-process guard for the whole run.
"""
import argparse, atexit, hashlib, json, os, platform, re, subprocess, tempfile, time
from pathlib import Path
REPO=Path(__file__).resolve().parents[3]
BIN=REPO/'target/release-perf/tokenzero'
OUT=Path(__file__).with_suffix('').with_name('large_read_rss')
GUARD=Path('/tmp/zerostack-heavy-process.guard')
SIZES=(1<<20,10<<20,100<<20)
BUDGET=768<<20
TRE=re.compile(r'^[ 	]*([0-9.]+) real[ 	]+([0-9.]+) user[ 	]+([0-9.]+) sys[ 	]*$',re.M)
RRE=re.compile(r'^[ 	]*([0-9]+)[ 	]+maximum resident set size[ 	]*$',re.M)

def acquire(command):
 while True:
  try: GUARD.mkdir(); break
  except FileExistsError:
   try: pid=int((GUARD/'pid').read_text().strip()); os.kill(pid,0)
   except (FileNotFoundError,ValueError,ProcessLookupError):
    for p in GUARD.iterdir():
     if p.is_file(): p.unlink()
    GUARD.rmdir(); continue
   time.sleep(1)
 for name,value in {'pid':str(os.getpid()),'repository':str(REPO),'command':command,'started_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())}.items():
  (GUARD/name).write_text(value+chr(10))

def release():
 if not GUARD.exists() or not (GUARD/'pid').exists() or (GUARD/'pid').read_text().strip()!=str(os.getpid()): return
 for p in GUARD.iterdir():
  if p.is_file(): p.unlink()
 GUARD.rmdir()

def metric(text,observed):
 t,r=TRE.search(text),RRE.search(text)
 if not t or not r: raise RuntimeError('unparsed time output: '+text[-2000:])
 real,user,system=map(float,t.groups())
 return {'wall_seconds':real,'observed_wall_seconds':round(observed,6),'user_seconds':user,'system_seconds':system,'cpu_seconds':user+system,'max_rss_bytes':int(r.group(1))}

def command(path,cache,root):
 return [str(BIN),'read',str(path),'--allowed-root',str(root),'--cache-path',str(cache),'--json']

def single(path,cache,root):
 started=time.perf_counter(); p=subprocess.run(['/usr/bin/time','-l',*command(path,cache,root)],cwd=REPO,env={**os.environ,'LC_ALL':'C','LANG':'C'},stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True,timeout=300)
 if p.returncode: raise RuntimeError(f'read failed ({p.returncode}): {p.stderr[-2000:]}')
 return metric(p.stderr,time.perf_counter()-started)

def concurrent(path,tmp):
 started=time.perf_counter(); jobs=[]
 for i in range(4):
  timing=tmp/f'concurrent-{i}.time'; err=(tmp/f'concurrent-{i}.stderr').open('w')
  p=subprocess.Popen(['/usr/bin/time','-l','-o',str(timing),*command(path,tmp/f'concurrent-{i}.json',tmp)],cwd=REPO,env={**os.environ,'LC_ALL':'C','LANG':'C'},stdout=subprocess.DEVNULL,stderr=err,text=True)
  jobs.append((p,timing,err))
 samples=[]
 for p,timing,err in jobs:
  rc=p.wait(timeout=360); err.close()
  if rc: raise RuntimeError(f'concurrent read pid {p.pid} failed ({rc})')
  samples.append(metric(timing.read_text(),0))
 return {'processes':4,'observed_wall_seconds':round(time.perf_counter()-started,6),'max_process_rss_bytes':max(x['max_rss_bytes'] for x in samples),'sum_process_max_rss_bytes':sum(x['max_rss_bytes'] for x in samples),'ceiling_definition':'sum of /usr/bin/time -l per-process maxima; conservative because peaks may not coincide','per_process':samples}

def corpus(path,size):
 line=b'tokenzero-large-read:0123456789abcdef: deterministic UTF-8 payload\\n'; chunk=(line*(1048576//len(line)+1))[:1048576]
 with path.open('wb') as f:
  left=size
  while left: piece=chunk[:min(left,len(chunk))]; f.write(piece); left-=len(piece)

def digest(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  for part in iter(lambda:f.read(1048576),b''): h.update(part)
 return h.hexdigest()

def run(label,provenance):
 if not BIN.is_file(): raise SystemExit('target/release-perf/tokenzero is missing; never size-optimized --release for RSS/wall claims. Build: cargo build --profile release-perf -p tokenzero-cli --bin tokenzero --no-default-features')
 acquire(f'large_read_rss.py --label {label}'); atexit.register(release)
 try:
  with tempfile.TemporaryDirectory(prefix='tokenzero-large-read-') as raw:
   tmp=Path(raw); singles={}; biggest=None
   for size in SIZES:
    path=tmp/f'read-{size}.txt'; corpus(path,size); biggest=path
    singles[str(size)]=single(path,tmp/f'single-{size}.json',tmp)
   commit=subprocess.run(['git','rev-parse','HEAD'],cwd=REPO,capture_output=True,text=True,check=True).stdout.strip()
   result={'schema':'tokenzero.large-read-rss.v1','label':label,'environment':{'os':platform.platform(),'machine':platform.machine(),'logical_cpus':os.cpu_count(),'commit':commit,'binary':str(BIN.relative_to(REPO)),'binary_sha256':digest(BIN),'binary_mtime_ns':BIN.stat().st_mtime_ns,'binary_provenance':provenance,'time_command':'/usr/bin/time -l'},'method':{'corpus_bytes':list(SIZES),'corpus':'deterministic repeated UTF-8 lines; exact byte lengths','single_command':'tokenzero read <file> --json under /usr/bin/time -l; stdout discarded','concurrent_command':'four simultaneous /usr/bin/time -l tokenzero subprocesses, separate recovery caches','analysis':'One materialized UTF-8 payload is required for response processing; a second inline recovery payload is not required when path and exact range are stable. mmap was rejected because response ownership still materializes and truncation safety would add complexity without reducing that allocation.'},'workloads':{'single_reads':singles,'concurrent_4x_100mb':concurrent(biggest,tmp)},'budget':{'single_100mb_max_rss_bytes':BUDGET}}
   OUT.mkdir(parents=True,exist_ok=True); dest=OUT/f'{label}.json'; dest.write_text(json.dumps(result,indent=2,sort_keys=True)+chr(10)); return dest
 finally:
  release(); atexit.unregister(release)

def main():
 p=argparse.ArgumentParser(); g=p.add_mutually_exclusive_group(required=True); g.add_argument('--label',choices=('baseline','candidate')); g.add_argument('--check-budget',action='store_true'); p.add_argument('--binary-provenance',default='release-perf binary built from the working tree immediately before this measurement'); a=p.parse_args()
 if a.check_budget:
  path=run('budget-check',a.binary_provenance); actual=json.loads(path.read_text())['workloads']['single_reads'][str(SIZES[-1])]['max_rss_bytes']; path.unlink()
  if actual>BUDGET: raise SystemExit(f'single 100MB read RSS budget exceeded: {actual} > {BUDGET}')
  print(f'large-read RSS budget passed: {actual} <= {BUDGET}')
 else: print(run(a.label,a.binary_provenance).relative_to(REPO))
if __name__=='__main__': main()
