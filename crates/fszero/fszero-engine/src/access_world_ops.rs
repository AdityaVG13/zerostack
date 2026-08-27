//! `fs.world` access-ledger queries (hot / recent / coaccess).

use super::*;
use serde_json::{Value, json};

const DEFAULT_WORLD_QUERY_LIMIT: usize = 10;
const MAX_WORLD_QUERY_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldAccessQuery {
    Hot,
    Recent,
    Coaccess,
}

pub fn parse_world_access_query(q: &str) -> Option<WorldAccessQuery> {
    match q {
        "hot" => Some(WorldAccessQuery::Hot),
        "recent" => Some(WorldAccessQuery::Recent),
        "coaccess" => Some(WorldAccessQuery::Coaccess),
        _ => None,
    }
}

pub fn world_query_limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_WORLD_QUERY_LIMIT)
        .clamp(1, MAX_WORLD_QUERY_LIMIT)
}

impl FSZeroSession {
    pub fn record_access(&mut self, op: &str, path: &str, content_hash: &str) {
        if self.durable_degraded {
            return;
        }
        let ts = super::recovery::unix_epoch_secs();
        self.recovery
            .append_access_log(ts, op, path, content_hash, self.access_session_window);
    }

    pub fn record_search_hits_access(&mut self, payload: &str) {
        for line in payload.lines() {
            if line.is_empty() || line.starts_with("budget:") {
                continue;
            }
            let path = search_line_path(line);
            if !path.is_empty() {
                self.record_access("search", &path, "");
            }
        }
    }

    pub fn do_world_access_query(&mut self, args: &Value) -> String {
        let query = match args.get("query").and_then(Value::as_str) {
            Some(q) => q,
            None => return "world:0 (missing query)".to_string(),
        };
        let Some(kind) = parse_world_access_query(query) else {
            return super::op_result::op0("world", format!("unknown query: {query}"));
        };
        let limit = world_query_limit(args);
        let path_filter = args
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty());

        let rows = match kind {
            WorldAccessQuery::Hot => {
                let hot = self.recovery.query_access_hot(limit);
                hot.into_iter()
                    .filter(|(p, _)| path_filter.is_none_or(|f| p.contains(f)))
                    .map(|(path, count)| json!({"path": path, "count": count}))
                    .collect::<Vec<_>>()
            }
            WorldAccessQuery::Recent => {
                let recent = self.recovery.query_access_recent(limit);
                recent
                    .into_iter()
                    .filter(|(p, _)| path_filter.is_none_or(|f| p.contains(f)))
                    .map(|(path, last_ts)| json!({"path": path, "last_ts": last_ts}))
                    .collect::<Vec<_>>()
            }
            WorldAccessQuery::Coaccess => {
                let pairs = self.recovery.query_access_coaccess(limit);
                pairs
                    .into_iter()
                    .filter(|(a, b, _)| path_filter.is_none_or(|f| a.contains(f) || b.contains(f)))
                    .map(|(a, b, count)| json!({"pair": format!("{a}|{b}"), "count": count}))
                    .collect::<Vec<_>>()
            }
        };

        let body = json!({ "query": query, "rows": rows, "limit": limit, });
        let payload = body.to_string();
        let cref = self.recovery.put_content_ref(payload.as_bytes());
        self.recovery.put_key("world/access", payload.as_bytes());
        format!("world:1 query={query} ref={cref} rows={}", rows.len())
    }
}

fn search_line_path(line: &str) -> String {
    for prefix in ["DEF: ", "IMPORT: ", "CALLER: "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest.split(':').next().unwrap_or("").trim().to_string();
        }
    }

    if let Some((fk, _)) = line.split_once(':') {
        if fk.contains('/') || fk.ends_with(".rs") {
            return fk.trim().to_string();
        }
    }
    String::new()
}
