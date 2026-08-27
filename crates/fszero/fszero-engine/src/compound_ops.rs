use super::ast::relative_file_key;
use super::memory::{
    decode_wire_path, delete_memory, get_memory, list_memory, put_memory, rename_memory,
};
use super::*;

#[inline]
fn mem0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("mem", detail)
}

fn pick_compound_read_rel(session: &FSZeroSession, root: &Path) -> Option<String> {
    let main_rel = "src/main.rs";
    if root.join(main_rel).is_file() {
        return Some(main_rel.to_string());
    }
    let mut rels: Vec<String> = session
        .caches
        .content
        .keys()
        .map(|p| relative_file_key(root, p))
        .collect();
    rels.sort();
    if let Some(rel) = rels.into_iter().next() {
        return Some(rel);
    }
    let mut root_files: Vec<String> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_file())
                .map(|_| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    root_files.sort();
    root_files.into_iter().next()
}

fn pick_compound_entry_symbol(session: &FSZeroSession) -> String {
    if session.index.symbols.iter().any(|(n, _)| n == "main") {
        return "main".to_string();
    }
    session
        .index
        .symbols
        .first()
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| "main".to_string())
}

impl FSZeroSession {
    /// Durable memory volume ops (no daemon). Specs:
    /// - `put:path|bytes` / `mem:put:path|bytes` (via compound intent)
    /// - `get:path` — ref-first (no body in detail; expand `memory` / content ref)
    /// - `ls:` / `ls:prefix` — listing body parked under recovery key `memory/ls`
    ///   (distinct from `memory` so ls cannot clobber a prior get)
    /// - `delete:path`
    /// - `rename:from|to`
    ///
    /// Shared by MCP (`fszero.memory_*`), CodeMode (`fs.memory.*`), and
    /// compound `mem:` intents — one implementation, ref-first acks.
    pub fn do_memory(&mut self, spec: &str) -> String {
        if let Some(rest) = spec.strip_prefix("put:") {
            let Some((path_enc, content)) = rest.split_once('|') else {
                return mem0("put requires path|content");
            };
            let path = decode_wire_path(path_enc.trim());
            return match put_memory(&mut self.recovery, &path, content.as_bytes()) {
                Ok(r) => {
                    self.recovery.put_key("memory", content.as_bytes());
                    format!("mem:1 put {path} ref={r}")
                }
                Err(e) => mem0(e),
            };
        }
        if let Some(path) = spec.strip_prefix("get:") {
            return match get_memory(&self.recovery, path.trim()) {
                Ok(bytes) => {
                    let r = self.recovery.put_payload_at_key("memory", &bytes);
                    format!("mem:1 get {} bytes={} ref={r}", path.trim(), bytes.len())
                }
                Err(e) => mem0(e),
            };
        }
        if let Some(path) = spec.strip_prefix("delete:") {
            return match delete_memory(&mut self.recovery, path.trim()) {
                Ok(()) => format!("mem:1 delete {}", path.trim()),
                Err(e) => mem0(e),
            };
        }
        if let Some(rest) = spec.strip_prefix("rename:") {
            let Some((from_enc, to_enc)) = rest.split_once('|') else {
                return mem0("rename requires from|to");
            };
            let from = decode_wire_path(from_enc.trim());
            let to = decode_wire_path(to_enc.trim());
            return match rename_memory(&mut self.recovery, &from, &to) {
                Ok(new_path) => format!("mem:1 rename {from} -> {new_path}"),
                Err(e) => mem0(e),
            };
        }
        if let Some(prefix) = spec.strip_prefix("ls:") {
            return self.memory_ls(prefix.trim());
        }
        if spec == "ls" || spec.is_empty() {
            return self.memory_ls("");
        }
        mem0("use put:path|content, get:path, delete:path, rename:from|to, or ls:[prefix]")
    }

    fn memory_ls(&mut self, prefix: &str) -> String {
        let listed = list_memory(&self.recovery, prefix);
        self.recovery
            .put_key("memory/ls", listed.join("\n").as_bytes());
        format!("mem:1 ls count={}", listed.len())
    }

    pub fn do_compound(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let intent = arg.unwrap_or("default-build-seq");
        if let Some(mem_spec) = intent.strip_prefix("mem:") {
            return self.do_memory(mem_spec);
        }
        let cache_key = (intent.to_string(), self.index.ast_generation);
        if let Some(summary) = self.caches.compound.get(&cache_key) {
            self.views.last_compound_payload = Some(summary.as_bytes().to_vec());
            return summary.clone();
        }
        if let Err(e) = self.prepare_index_or_busy(root) {
            return e;
        }
        if root.is_some() {
            if let Some(msg) = super::search::files_budget_message(self.indexed_file_count()) {
                if let Some((_, cap, scanned)) = super::search::parse_budget_message(&msg) {
                    self.store_budget_evidence("C", "files", cap, scanned);
                }
                return msg;
            }
        }
        let read_rel = match root.and_then(|r| pick_compound_read_rel(self, r)) {
            Some(rel) => rel,
            None => return "compound:0 no indexed text file in repo".to_string(),
        };
        let entry_sym = pick_compound_entry_symbol(self);
        // Internal: 1 ack for bundle. Predictive prefetch on read, ast queries for structural.
        let _ = self.do_ls(root, None);
        let _ = self.do_search(root, Some("fn "));
        let read_result = self.do_read(root, Some(&read_rel));
        if read_result.starts_with("bad ") || read_result.starts_with("read:0") {
            return format!("compound:0 required read failed: {read_result}");
        }
        let callers_q = format!("callers:{entry_sym}");
        let _ = self.do_search(root, Some(&callers_q));
        let summary = if intent == "matrix" {
            "compound:4 cached ops for intent matrix".to_string()
        } else {
            format!("compound:5+ ops (ast+cache) path={read_rel} for intent {intent}")
        };
        self.finish_compound_summary(cache_key, summary)
    }

    fn finish_compound_summary(&mut self, cache_key: (String, u64), summary: String) -> String {
        self.caches.compound.insert(cache_key, summary.clone());
        self.views.last_compound_payload = Some(summary.as_bytes().to_vec());
        self.recovery.put_key("compound", summary.as_bytes());
        if let Some(err) = self.store_error_suffix("compound") {
            return err;
        }
        summary
    }
}
