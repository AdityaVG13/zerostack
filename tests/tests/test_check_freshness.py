from __future__ import annotations
import importlib.util, json, tempfile, unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
SCRIPT=Path(__file__).parents[1]/"scripts"/"check_freshness.py"
SPEC=importlib.util.spec_from_file_location("check_freshness",SCRIPT); assert SPEC and SPEC.loader
freshness=importlib.util.module_from_spec(SPEC); SPEC.loader.exec_module(freshness)
NOW=datetime(2026,7,28,12,tzinfo=timezone.utc)
class FreshnessTests(unittest.TestCase):
    def setUp(self)->None:
        self.temp=tempfile.TemporaryDirectory(); self.root=Path(self.temp.name)
        self.report={"engine":"fz","surface":"codemode","semantic_contract_digest":"a"*64,"operation_registry_digest":"b"*64,"git_revision":"c"*40,"timestamp":NOW.isoformat().replace("+00:00","Z"),"bin":"fszero-codemode","passed":True,"completion_status":"complete"}
        self.entry={"report":"fz.json",**{key:self.report[key] for key in freshness.FIELDS}}
        self.write("fz.json",self.report); self.write("attestation.json",{"reports":[self.entry]})
    def tearDown(self)->None: self.temp.cleanup()
    def write(self,name:str,value:object)->None: (self.root/name).write_text(json.dumps(value),encoding="utf-8")
    def errors(self)->list[str]: return freshness.validate(self.root/"attestation.json",30,NOW)
    def rewrite(self)->None: self.write("fz.json",self.report); self.write("attestation.json",{"reports":[self.entry]})
    def test_valid(self)->None: self.assertEqual(self.errors(),[])
    def test_missing_report(self)->None:
        (self.root/"fz.json").unlink(); self.assertTrue(any("cannot read" in x for x in self.errors()))
    def test_stale_report(self)->None:
        old=(NOW-timedelta(days=31)).isoformat().replace("+00:00","Z"); self.report["timestamp"]=old; self.entry["timestamp"]=old; self.rewrite(); self.assertTrue(any("stale" in x for x in self.errors()))
    def test_mismatched_content(self)->None:
        self.report["surface"]="mcp"; self.write("fz.json",self.report); self.assertTrue(any("surface does not match" in x for x in self.errors()))
    def test_absolute_bin_path(self)->None:
        self.report["bin"]="/opt/bin/fszero"; self.entry["bin"]=self.report["bin"]; self.rewrite(); self.assertTrue(any("bin must be basename-only" in x for x in self.errors()))
    def test_partial_or_failed_report(self)->None:
        self.report.update(passed=False,completion_status="partial"); self.write("fz.json",self.report); errors=self.errors(); self.assertTrue(any("passed must be true" in x for x in errors)); self.assertTrue(any("completion_status" in x for x in errors))
    def test_unindexed_extra(self)->None:
        self.write("extra.json",self.report); self.assertTrue(any("extra.json: report is not indexed" in x for x in self.errors()))
if __name__=="__main__": unittest.main()
