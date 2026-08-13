from __future__ import annotations
import copy
import json
import re
import tomllib
import unittest
from pathlib import Path
from typing import Any
ROOT=Path(__file__).parents[1]
SCHEMA_PATH=ROOT/'contracts/engine-topology-v1.schema.json'
MANIFEST_PATH=ROOT.parent/'conformance/engine-topology-v1.json'
class SchemaFailure(AssertionError): pass
def resolve(s:dict[str,Any], root:dict[str,Any])->dict[str,Any]:
    ref=s.get('$ref')
    if not ref: return s
    if not ref.startswith('#/'): raise SchemaFailure(f'external ref: {ref}')
    value:Any=root
    for part in ref[2:].split('/'): value=value[part]
    return value
def validate(value:Any,schema:dict[str,Any],root:dict[str,Any],path:str='$')->None:
    schema=resolve(schema,root)
    if 'const' in schema and value!=schema['const']: raise SchemaFailure(f'{path}: const')
    if 'enum' in schema and value not in schema['enum']: raise SchemaFailure(f'{path}: enum')
    types=schema.get('type',[]); types=types if isinstance(types,list) else [types]
    if types:
        ok={'object':isinstance(value,dict),'array':isinstance(value,list),'string':isinstance(value,str),'boolean':isinstance(value,bool),'null':value is None,'integer':isinstance(value,int) and not isinstance(value,bool),'number':isinstance(value,(int,float)) and not isinstance(value,bool)}
        if not any(ok.get(t,False) for t in types): raise SchemaFailure(f'{path}: type')
    if isinstance(value,str):
        if len(value)<schema.get('minLength',0): raise SchemaFailure(f'{path}: minLength')
        if 'pattern' in schema and re.search(schema['pattern'],value) is None: raise SchemaFailure(f'{path}: pattern')
    if isinstance(value,list):
        if len(value)<schema.get('minItems',0) or len(value)>schema.get('maxItems',len(value)): raise SchemaFailure(f'{path}: item count')
        if schema.get('uniqueItems') and len({json.dumps(x,sort_keys=True) for x in value})!=len(value): raise SchemaFailure(f'{path}: duplicates')
        if 'items' in schema:
            for i,item in enumerate(value): validate(item,schema['items'],root,f'{path}[{i}]')
    if isinstance(value,dict):
        missing=[x for x in schema.get('required',[]) if x not in value]
        if missing: raise SchemaFailure(f'{path}: missing {missing}')
        props=schema.get('properties',{})
        if schema.get('additionalProperties') is False:
            unknown=set(value)-set(props)
            if unknown: raise SchemaFailure(f'{path}: unknown {sorted(unknown)}')
        for key,sub in props.items():
            if key in value: validate(value[key],sub,root,f'{path}.{key}')
def load(path:Path)->dict[str,Any]:
    with path.open(encoding='utf-8') as handle: value=json.load(handle)
    if not isinstance(value,dict): raise AssertionError(path)
    return value
def key(item:dict[str,Any])->str: return f"{item['path']}:{item['package']}:{item['name']}"
EXPECTED={
 'tokenzero':{'crates/tokenzero-core/Cargo.toml:tokenzero-core:tokenzero-core','crates/tokenzero-recovery/Cargo.toml:tokenzero-recovery:tokenzero-recovery','crates/tokenzero-runtime/Cargo.toml:tokenzero-runtime:tokenzero-runtime','crates/tokenzero-engine/Cargo.toml:tokenzero-engine:tokenzero-engine','crates/tokenzero-filters/Cargo.toml:tokenzero-filters:tokenzero-filters','crates/tokenzero-codemode/Cargo.toml:tokenzero-worker:tokenzero-worker','crates/tokenzero-mcp-compat/Cargo.toml:tokenzero-mcp-compat:tokenzero-mcp-compat','crates/tokenzero/Cargo.toml:tokenzero-cli:tokenzero-cli','crates/tokenzero/src/bin/tokenzero_mcp.rs:tokenzero-cli:tokenzero-mcp','crates/tokenzero-install/Cargo.toml:tokenzero-install:tokenzero-install','crates/tokenzero-pulse/Cargo.toml:tokenzero-pulse:tokenzero-pulse','crates/tokenzero-test-support/Cargo.toml:tokenzero-test-support:tokenzero-test-support','fuzz/Cargo.toml:tokenzero-fuzz:tokenzero-fuzz','crates/tokenzero/src/main.rs:tokenzero-cli:tokenzero','crates/tokenzero/src/main.rs:tokenzero-cli:tokenzero-cli','crates/tokenzero-codemode/src/main.rs:tokenzero-worker:tokenzero-codemode','crates/tokenzero-codemode/src/main.rs:tokenzero-worker:tokenzero-worker','fuzz/fuzz_targets/expand_fragment_differential.rs:tokenzero-fuzz:expand_fragment_differential'},
 'fszero':{'Cargo.toml:fs-zero:fs-zero','crates/fszero-codemode/Cargo.toml:fszero-worker:fszero-worker','crates/fszero-codemode/src/main.rs:fszero-worker:fszero-codemode','crates/fszero-codemode/src/main.rs:fszero-worker:fszero-worker','crates/fszero-mcp/Cargo.toml:fszero-mcp:fszero-mcp','crates/fszero-mcp/src/main.rs:fszero-mcp:fszero-mcp','crates/fszero-shim/Cargo.toml:fszero-cli:fszero-cli','crates/fszero-shim/src/main.rs:fs-zero:fszero','crates/fszero-shim/src/main.rs:fszero-cli:fszero','crates/fszero-shim/src/main.rs:fszero-cli:fszero-cli','crates/fszero-test-support/Cargo.toml:fszero-test-support:fszero-test-support','xtask/Cargo.toml:zerostack-xtask:zerostack-xtask','xtask/src/main.rs:zerostack-xtask:zerostack-xtask'},
 'graphzero':{'benchmarks/foreign_corpora/fixtures/rust-mini/Cargo.toml:rust-mini:rust-mini','benchmarks/foreign_corpora/fixtures/rust-mini/src/main.rs:rust-mini:rust-mini','crates/graphzero-cli/Cargo.toml:graphzero-cli:graphzero-cli','crates/graphzero-cli/src/bin/graphzero_mcp.rs:graphzero-cli:graphzero-mcp','crates/graphzero-cli/src/main.rs:graphzero-cli:graphzero','crates/graphzero-cli/src/main.rs:graphzero-cli:graphzero-cli','crates/graphzero-codemode/Cargo.toml:graphzero-worker:graphzero-worker','crates/graphzero-codemode/src/main.rs:graphzero-worker:graphzero-codemode','crates/graphzero-codemode/src/main.rs:graphzero-worker:graphzero-worker','crates/graphzero-core/Cargo.toml:graphzero-core:graphzero-core','crates/graphzero-coverage/Cargo.toml:graphzero-coverage:graphzero-coverage','crates/graphzero-extract/Cargo.toml:graphzero-extract:graphzero-extract','crates/graphzero-mcp-compat/Cargo.toml:graphzero-mcp-compat:graphzero-mcp-compat','crates/graphzero-mcp-compat/src/main.rs:graphzero-mcp-compat:graphzero-mcp-compat','crates/graphzero-pack/Cargo.toml:graphzero-pack:graphzero-pack','crates/graphzero-query/Cargo.toml:graphzero-query:graphzero-query','crates/graphzero-query/src/bin/gz_raw_worker.rs:graphzero-query:gz-raw-worker','crates/graphzero-query/src/bin/gz_surface_bench_worker.rs:graphzero-query:gz-surface-bench-worker','crates/graphzero-query/src/bin/gzero.rs:graphzero-query:gzero','crates/graphzero-reserve/Cargo.toml:graphzero-reserve:graphzero-reserve','crates/graphzero-scip/Cargo.toml:graphzero-scip:graphzero-scip','crates/graphzero-scip/tools_gen/Cargo.toml:gen-fixture:gen-fixture','crates/graphzero-scip/tools_gen/src/main.rs:gen-fixture:gen-fixture','crates/graphzero-semantic/Cargo.toml:graphzero-semantic:graphzero-semantic','crates/graphzero-store/Cargo.toml:graphzero-store:graphzero-store','crates/graphzero-test-support/Cargo.toml:graphzero-test-support:graphzero-test-support','crates/graphzero-types/Cargo.toml:graphzero-types:graphzero-types','crates/graphzero-why/Cargo.toml:graphzero-why:graphzero-why','fuzz/Cargo.toml:graphzero-fuzz:graphzero-fuzz','fuzz/fuzz_targets/delta_codec.rs:graphzero-fuzz:delta_codec','fuzz/fuzz_targets/pack_sign.rs:graphzero-fuzz:pack_sign','fuzz/fuzz_targets/scip_parse.rs:graphzero-fuzz:scip_parse'}
}

SIBLING_ROOTS={"tokenzero":ROOT.parents[1]/"TokenZero","fszero":ROOT.parents[1]/"FSZero","graphzero":ROOT.parents[1]/"GraphZero"}
RAW_WORKER_PROHIBITIONS=frozenset({"planner","QuickJS-runtime","MCP-catalog","harness-discovery","nested-CodeMode","Pi-routing","unbounded-child-process","hidden-async-tail","journal-bypass","cancellation-bypass","RACC-bypass","typed-failure-bypass","private-protocol"})

def enumerate_live_inventory(root:Path)->set[str]:
    actual:set[str]=set()
    for manifest_path in sorted(root.rglob("Cargo.toml")):
        if any(part in {".git","target"} for part in manifest_path.parts): continue
        data=tomllib.loads(manifest_path.read_text(encoding="utf-8")); package=data.get("package")
        if not package or not package.get("name"): continue
        package_name=package["name"]; manifest_rel=manifest_path.relative_to(root).as_posix()
        actual.add(f"crate:{manifest_rel}:{package_name}:{package_name}")
        binaries=[]
        if (manifest_path.parent/"src/main.rs").is_file(): binaries.append((package_name,"src/main.rs"))
        binaries.extend((decl.get("name",package_name),decl.get("path","src/main.rs")) for decl in data.get("bin",[]))
        for binary_name,source_path in binaries:
            source=(manifest_path.parent/source_path).resolve().relative_to(root.resolve()).as_posix()
            actual.add(f"binary:{source}:{package_name}:{binary_name}")
    return actual

class TopologyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls)->None:
        cls.schema,cls.manifest=load(SCHEMA_PATH),load(MANIFEST_PATH)
    def test_schema_refs_and_manifest(self)->None:
        self.assertEqual(self.schema['$schema'],'https://json-schema.org/draft/2020-12/schema'); self.assertEqual(self.schema['$id'],'https://zerostack.local/schemas/engine-topology-v1.schema.json')
        refs=[]
        def collect(v:Any)->None:
            if isinstance(v,dict):
                if '$ref' in v: refs.append(v['$ref'])
                for x in v.values(): collect(x)
            elif isinstance(v,list):
                for x in v: collect(x)
        collect(self.schema)
        for ref in refs:
            self.assertTrue(ref.startswith('#/')); value:Any=self.schema
            for part in ref[2:].split('/'): self.assertIn(part,value); value=value[part]
        validate(self.manifest,self.schema,self.schema)
    def test_schema_reuses_command_definition(self)->None:
        ref={"$ref":"#/$defs/command"}
        self.assertEqual(self.schema["properties"]["hub"]["properties"]["build_commands"]["items"],ref)
        self.assertEqual(self.schema["properties"]["engines"]["items"]["properties"]["target"]["properties"]["build_commands"]["items"],ref)
        self.assertIn("env",self.schema["$defs"]["command"]["required"])

    def test_inventory_is_complete(self)->None:
        for engine in self.manifest["engines"]:
            actual={key(item) for item in engine["current_to_target"]}
            self.assertEqual(actual,EXPECTED[engine["id"]])
            self.assertEqual(len(actual),len(engine["current_to_target"]))
            sibling=SIBLING_ROOTS[engine["id"]]
            if (sibling/"Cargo.toml").is_file():
                live={f"{item['kind']}:{key(item)}" for item in engine["current_to_target"]}
                self.assertEqual(live,enumerate_live_inventory(sibling),engine["id"])

    def test_build_commands_are_derivable(self)->None:
        def assert_command(command:dict[str,Any],cwd:str)->None:
            self.assertEqual(command["runner"],"rch")
            self.assertEqual(command["program"],"cargo")
            self.assertEqual(command["working_directory"],cwd)
            self.assertEqual(command["env"],{"CARGO_TARGET_DIR":f".rch-target/{cwd}"})
            self.assertEqual(set(command),{"id","runner","program","args","working_directory","purpose","env"})
        hub={x["id"]:x for x in self.manifest["hub"]["build_commands"]}
        self.assertEqual(hub["hub-workspace-check"]["args"],["check","--locked","--workspace","--all-targets"])
        self.assertEqual(hub["hub-zsx-release"]["args"],["build","--locked","--release","-p","zsx","--bin","zsx"])
        binaries={x["name"]:x for x in self.manifest["hub"]["binaries"]}
        self.assertEqual(binaries["zsx"]["path"],"crates/zsx/src/main.rs")
        for command in hub.values(): assert_command(command,"zerostack")
        for engine in self.manifest["engines"]:
            target=engine["target"]; commands={x["id"]:x for x in target["build_commands"]}
            self.assertEqual(set(commands),{"workspace-check","cli-release","worker-release","test-support-check"})
            self.assertEqual(commands["workspace-check"]["args"],["check","--locked","--workspace","--all-targets"])
            self.assertEqual(commands["cli-release"]["args"],["build","--locked","--release","-p",target["cli_package"],"--bin",target["cli_binary"]])
            self.assertEqual(commands["worker-release"]["args"],["build","--locked","--release","-p",target["worker_package"],"--bin",target["worker_binary"],"--no-default-features"])
            self.assertEqual(commands["test-support-check"]["args"],["test","--locked","-p",target["test_support_package"],"--no-run"])
            for command in commands.values(): assert_command(command,engine["id"])

    def test_worker_boundary_and_adapters(self)->None:
        raw=self.manifest["raw_worker_v2"]
        self.assertEqual(set(raw["prohibitions"]),RAW_WORKER_PROHIBITIONS)
        # Prohibition data lives in tracked conformance authority
        # conformance/engine-topology-v1.json (raw_worker_v2.prohibitions,
        # asserted above). docs/adr/ is intentionally untracked (gitignored
        # per 2db9d25 "docs: keep architecture decisions private"), so the
        # test must not read the ADR from a clean checkout.
        self.assertEqual(raw["protocol"],"raw-worker-v2")
        self.assertEqual(raw["runtime_owner"],"zero-codemode-legacy-conformance")
        self.assertEqual(raw["skew_policy"],"fail-closed")
        policy=self.manifest["canonical_engine_skeleton"]["feature_policy"]
        self.assertEqual(policy["worker_default_features"],[])
        for item in ("quickjs","js","surface-mcp","mcp-catalog","planner","nested-codemode","harness-routing"): self.assertIn(item,policy["worker_forbidden_features"])
        self.assertEqual({x["kind"] for x in self.manifest["harness_adapters"]},{"plain-cli","mcp-claude-code","pi","omp","third-party"})
        adapters={x["id"]:x for x in self.manifest["harness_adapters"]}
        self.assertEqual(adapters["plain-cli"]["entrypoint"],"zsx")
        self.assertEqual(adapters["mcp-claude-code"]["entrypoint"],"zero-mcp")
        self.assertEqual(adapters["pi"]["entrypoint"],"@zerostack/zsx-native")
        self.assertEqual(adapters["omp"]["entrypoint"],"@zerostack/zsx-native")
        self.assertTrue(all(x["status"]=="thin-adapter" for x in adapters.values()))
        self.assertEqual(self.manifest["canonical_engine_skeleton"]["host_client_contract"]["name"],"zsx-native-session-v1")

    def test_canonical_migration_quality_gate_is_uniform(self)->None:
        policy=self.manifest["canonical_engine_skeleton"]["quality_gate_policy"]
        self.assertEqual(policy["repositories"],["zerostack","tokenzero","fszero","graphzero"])
        self.assertEqual(policy["runner"],"rch")
        self.assertEqual(policy["rustfmt_args"],["fmt","--all","--","--check"])
        self.assertEqual(policy["clippy_args_template"],["clippy","-p","<package>","--all-targets","--no-deps","--","-D","warnings"])
        self.assertEqual(policy["activation"],"required-before-canonical-migration")
        self.assertEqual(policy["existing_debt_policy"],"does-not-weaken-migration-gate")

    def test_rejects_nonportable_paths_with_mutations(self)->None:
        bad_paths=["/tmp/root","../outside","a/../../outside","C:\\repo","\\\\server\\share","a\\b"]
        for bad in bad_paths:
            altered=copy.deepcopy(self.manifest); altered["engines"][0]["current_to_target"][0]["path"]=bad
            with self.assertRaises(SchemaFailure): validate(altered,self.schema,self.schema)
            altered=copy.deepcopy(self.manifest); altered["hub"]["build_commands"][0]["working_directory"]=bad
            with self.assertRaises(SchemaFailure): validate(altered,self.schema,self.schema)
            altered=copy.deepcopy(self.manifest); altered["hub"]["build_commands"][0]["env"]["CARGO_TARGET_DIR"]=bad
            with self.assertRaises(SchemaFailure): validate(altered,self.schema,self.schema)

    def test_no_host_paths(self)->None:
        def walk(v:Any)->list[str]:
            if isinstance(v,str): return [v]
            if isinstance(v,list): return [y for x in v for y in walk(x)]
            if isinstance(v,dict): return [y for x in v.values() for y in walk(x)]
            return []
        for value in walk(self.manifest): self.assertFalse(value.startswith(('/', '~')) or '/Users/' in value or '\\\\Users\\\\' in value,value)
if __name__=='__main__': unittest.main()
