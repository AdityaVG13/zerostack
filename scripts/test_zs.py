#!/usr/bin/env python3
from __future__ import annotations
import json, os, stat, subprocess, sys, tempfile, unittest
from pathlib import Path
HERE = Path(__file__).resolve().parent
ZS, INSTALLER = HERE / "zs", HERE / "install_zs.py"
FAKE = '''#!/usr/bin/env python3
import json, os, sys
if len(sys.argv) > 1 and sys.argv[1] == "catalog":
    if os.environ.get("FAKE_CATALOG_EXIT"):
        print("catalog failed", file=sys.stderr); raise SystemExit(int(os.environ["FAKE_CATALOG_EXIT"]))
    print(json.dumps({"methods":[
        {"name":"fs.read","signature":"fs.read(path: string)","description":"Read file contents","surface":"codemode"},
        {"name":"fs.search","signature":"fs.search(query: string)","description":"Search files","surface":"codemode"},
        {"name":"graph.read","signature":"graph.read()","description":"Wrong engine","surface":"codemode"},
        {"name":"fs_mcp_read","description":"MCP adapter method","surface":"mcp"}
    ]})); raise SystemExit(0)
if os.environ.get("FAKE_EXIT"): raise SystemExit(int(os.environ["FAKE_EXIT"]))
messages=[json.loads(line) for line in sys.stdin if line.strip()]
call=next(x for x in messages if x.get("id")==2)
with open(os.environ["FAKE_LOG"],"w") as log: json.dump({"cwd":os.getcwd(),"call":call},log)
print(json.dumps({"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"fakezero","version":"9.1","revision":"abc123"}}}))
name=call["params"]["name"]
if name.endswith("_search"):
    structured={"status":"S2"} if os.environ.get("FAKE_STATUS_ONLY") else {"status":"S2","methods":[{"rank":1,"name":"fs.read","detail":"Read a file"}]}
    result={"content":[{"type":"text","text":"S2"}],"structuredContent":structured}
elif name.endswith("_describe"): result={"content":[{"type":"text","text":"fs.read(path): file contents"}]}
elif call["params"]["arguments"].get("envelope") == "v1":
    result={"content":[{"type":"text","text":"R2"}],"structuredContent":{"result":{"ack":"R2","ref":"fz://blob/deadbeef","result":{"payload":"expanded contents"},"more":["gz://node/1","tz://blob/2","cm://exec/3"]}}}
else: result={"content":[{"type":"text","text":"R2"}],"structuredContent":{"ack":"R2","ref":"fz://blob/deadbeef","more":["gz://node/1","tz://blob/2","cm://exec/3"]}}
print(json.dumps({"jsonrpc":"2.0","id":2,"result":result}))
'''
class ZsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp=tempfile.TemporaryDirectory(); self.root=Path(self.tmp.name); self.fake=self.root/"engine"
        self.fake.write_text(FAKE); self.fake.chmod(0o755); self.log=self.root/"log.json"
        self.env=os.environ|{key:str(self.fake) for key in ("ZS_FSZERO_BIN","ZS_GRAPHZERO_BIN","ZS_TOKENZERO_BIN")}|{"FAKE_LOG":str(self.log)}
    def tearDown(self) -> None: self.tmp.cleanup()
    def run_zs(self,*args:str,input_text:str|None=None)->subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable,str(ZS),*args],input=input_text,text=True,capture_output=True,env=self.env)
    def test_version(self)->None:
        result=self.run_zs("--version"); self.assertEqual(result.returncode,0); self.assertRegex(result.stdout,r"^zs \d+\.\d+\.\d+")
    def test_root_and_stdin_plan(self)->None:
        result=self.run_zs("-C",str(self.root),"fs","-",input_text="return 7;"); self.assertEqual(result.returncode,0,result.stderr)
        logged=json.loads(self.log.read_text()); self.assertEqual(logged["cwd"],str(self.root.resolve())); self.assertEqual(logged["call"]["params"]["arguments"]["plan"],"return 7;")
    def test_inline_discovery_and_scalar_ref(self)->None:
        search=self.run_zs("fs-search","read file"); self.assertIn("fs.read",search.stdout); self.assertIn("Read a file",search.stdout)
        self.assertIn("fs.read(path)",self.run_zs("fs-describe","fs.read").stdout)
        execute=self.run_zs("fs","return 1"); self.assertIn("R2",execute.stdout); self.assertIn("expanded contents",execute.stdout); self.assertIn("fz://blob/deadbeef",execute.stdout)
        self.assertEqual(json.loads(self.log.read_text())["call"]["params"]["arguments"]["envelope"], "v1")
        for ref in ("gz://node/1", "tz://blob/2", "cm://exec/3"): self.assertIn(ref, execute.stdout)
    def test_status_only_search_falls_back_to_ranked_catalog(self)->None:
        self.env["FAKE_STATUS_ONLY"]="1"
        result=self.run_zs("-C",str(self.root),"fs-search","read file")
        self.assertEqual(result.returncode,0,result.stderr); self.assertIn("fs.read(path: string)",result.stdout); self.assertIn("Read file contents",result.stdout)
        self.assertNotIn("graph.read",result.stdout); self.assertNotEqual(result.stdout.strip(),"S2")
    def test_catalog_failure_preserves_exit_evidence(self)->None:
        self.env.update({"FAKE_STATUS_ONLY":"1","FAKE_CATALOG_EXIT":"19"})
        result=self.run_zs("fs-search","read file")
        self.assertEqual(result.returncode,19); self.assertIn("catalog exited with status 19: catalog failed",result.stderr)
    def test_json_and_verbose_metadata(self)->None:
        result=self.run_zs("--json","--verbose","fs","return 1"); self.assertIn("structuredContent",result.stdout); self.assertIn("zs 1.2.0; engine fakezero 9.1; revision abc123",result.stderr)
        self.assertNotIn("envelope",json.loads(self.log.read_text())["call"]["params"]["arguments"])
    def test_error_has_copyable_root_guidance(self)->None:
        result=self.run_zs("-C",str(self.root),"bogus","x"); self.assertEqual(result.returncode,2); self.assertIn(f"Rerun: zs -C {self.root.resolve()} bogus x",result.stderr); self.assertIn("Use paths relative to this root",result.stderr)
    def test_engine_exit_status_is_preserved(self)->None:
        env=self.env|{"FAKE_EXIT":"23"}
        result=subprocess.run([sys.executable,str(ZS),"fs","return 1"],text=True,capture_output=True,env=env)
        self.assertEqual(result.returncode,23)

    def test_installer_atomic_dry_run_and_verify(self)->None:
        prefix=self.root/"bin"; command=[sys.executable,str(INSTALLER),"--prefix",str(prefix)]
        dry=subprocess.run([*command,"--dry-run"],text=True,capture_output=True); self.assertEqual(dry.returncode,0); self.assertFalse((prefix/"zs").exists())
        installed=subprocess.run(command,text=True,capture_output=True); self.assertEqual(installed.returncode,0,installed.stdout+installed.stderr)
        target=prefix/"zs"; self.assertEqual(target.read_bytes(),ZS.read_bytes()); self.assertTrue(target.stat().st_mode&stat.S_IXUSR); self.assertFalse(list(prefix.glob(".zs.*")))
        self.assertEqual(subprocess.run([*command,"--verify"],text=True,capture_output=True).returncode,0)
if __name__=="__main__": unittest.main()
