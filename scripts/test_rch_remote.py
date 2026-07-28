from __future__ import annotations
import json, os
from pathlib import Path
import subprocess as sp
import sys, tempfile, textwrap, unittest
WRAPPER=Path(__file__).with_name("rch_remote.py")
class Tests(unittest.TestCase):
 def setUp(self)->None:
  self.tmp=tempfile.TemporaryDirectory(); self.base=Path(self.tmp.name); self.root=self.base/"canonical root"; self.work=self.root/"project with spaces"; self.outside=self.base/"outside"; self.bin=self.base/"bin"; self.log=self.base/"calls"; self.sentinel=self.base/"LOCAL_COMPILER_RAN"
  for path in (self.work,self.outside,self.bin): path.mkdir(parents=True)
  fake=self.bin/"rch"; fake.write_text(textwrap.dedent("""\
   #!/usr/bin/env python3
   import json,os,pathlib,subprocess,sys
   log=pathlib.Path(os.environ["FAKE_LOG"])
   with log.open("a") as out: out.write(json.dumps(sys.argv[1:])+"\\n")
   if sys.argv[1:]==["config","show"]:
    print("[path_topology]"); print(f'  canonical_root = "{os.environ["FAKE_ROOT"]}"'); raise SystemExit
   mode=os.environ["FAKE_MODE"]
   strict=all(os.environ.get(k)=="true" for k in ("RCH_FORCE_REMOTE","RCH_REQUIRE_REMOTE","RCH_QUEUE_WHEN_BUSY"))
   if mode=="local":
    if not strict: pathlib.Path(os.environ["SENTINEL"]).write_text("ran"); subprocess.run(sys.argv[3:])
    print("[RCH] local (remote execution failed)"); raise SystemExit
   if mode=="busy": print("[RCH] queued (all workers busy)"); raise SystemExit(75)
   if mode=="failure": print("[RCH] remote fake-worker"); raise SystemExit(23)
   if mode=="unproved": print("compiler completed"); raise SystemExit
   print("[RCH] remote fake-worker")
  """)); fake.chmod(0o755)
 def tearDown(self)->None: self.tmp.cleanup()
 def invoke(self,mode:str,cwd:Path,*args:str)->sp.CompletedProcess[str]:
  env=os.environ.copy(); env.update(RCH_REMOTE_BIN=str(self.bin/"rch"),FAKE_LOG=str(self.log),FAKE_ROOT=str(self.root),FAKE_MODE=mode,SENTINEL=str(self.sentinel))
  return sp.run([sys.executable,str(WRAPPER),"--",*args],cwd=cwd,env=env,text=True,capture_output=True,check=False)
 def calls(self)->list[list[str]]: return [json.loads(x) for x in self.log.read_text().splitlines()] if self.log.exists() else []
 def test_remote_success_preserves_space_argv_without_shell(self)->None:
  r=self.invoke("success",self.work,"compiler name","arg with spaces","; touch NO"); self.assertEqual(r.returncode,0,r.stderr); self.assertIn("remote_success",r.stderr); self.assertEqual(self.calls()[-1],["exec","--","compiler name","arg with spaces","; touch NO"]); self.assertFalse((self.work/"NO").exists())
 def test_busy(self)->None:
  r=self.invoke("busy",self.work,"cc"); self.assertEqual(r.returncode,75); self.assertIn("queued_or_busy",r.stderr)
 def test_local_fallback_is_forbidden_and_compiler_never_runs(self)->None:
  r=self.invoke("local",self.work,"sentinel compiler"); self.assertNotEqual(r.returncode,0); self.assertIn("forbidden_local_fallback",r.stderr); self.assertFalse(self.sentinel.exists())
 def test_missing_remote_proof_fails(self)->None:
  r=self.invoke("unproved",self.work,"cargo","check"); self.assertEqual(r.returncode,70); self.assertIn("forbidden_local_fallback",r.stderr)
 def test_command_failure_exit_is_preserved(self)->None:
  r=self.invoke("failure",self.work,"cargo","test"); self.assertEqual(r.returncode,23); self.assertIn("remote_failure",r.stderr)
 def test_outside_root_rejected_before_exec(self)->None:
  r=self.invoke("success",self.outside,"cargo","build"); self.assertEqual(r.returncode,78); self.assertIn(f"resolved cwd={self.outside.resolve()}",r.stderr); self.assertIn(f"resolved root={self.root.resolve()}",r.stderr); self.assertIn("git -C",r.stderr); self.assertEqual(self.calls(),[["config","show"]])
if __name__=="__main__": unittest.main()
