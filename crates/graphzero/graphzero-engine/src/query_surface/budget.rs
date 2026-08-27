use super::QuerySurfaceRouter;
use super::types::*;
use serde::Serialize;
use serde_json::Value;
use std::io;

impl QuerySurfaceRouter {
    pub fn to_json_value(resp: &QuerySurfaceResponse) -> Value {
        serde_json::to_value(resp).expect("query surface response serializes")
    }

    pub fn to_json_string(resp: &QuerySurfaceResponse) -> String {
        Self::to_json_string_with_budget(resp, 1, None)
    }

    fn scrub_ref_first_hit_metadata(hits: &mut [SearchHit]) {
        hits.iter_mut().for_each(|hit| {
            hit.content_sha256.clear();
            hit.source.clear();
        });
    }

    fn spill_query_id(store_root: Option<&std::path::Path>, spill: &str) -> Option<String> {
        store_root
            .and_then(|root| graphzero_store::store::query::persist_query_json(root, spill).ok())
    }

    fn spill_ref_first_detail(
        resp: &mut QuerySurfaceResponse,
        store_root: Option<&std::path::Path>,
    ) {
        let detail_count =
            resp.edges.len() + resp.hits.len() + resp.outline.len() + resp.rows.len();
        if detail_count == 0 {
            return;
        }

        let spill = serde_json::to_string(&*resp).unwrap_or_default();
        let Some(id) = Self::spill_query_id(store_root, &spill) else {
            // Fail closed: never advertise a full_ref for bytes that were not persisted.
            return;
        };
        resp.full_ref = Some(format!("gz://query/{id}"));
        resp.edges.clear();
        resp.hits.clear();
        resp.outline.clear();
        resp.rows.clear();
        resp.truncated = Some(true);
    }

    fn apply_detail_cap(
        resp: &mut QuerySurfaceResponse,
        cap: usize,
        store_root: Option<&std::path::Path>,
        session: Option<&str>,
    ) {
        let rest_edges = if resp.edges.len() > cap {
            resp.edges.split_off(cap)
        } else {
            Vec::new()
        };
        let rest_hits = if resp.hits.len() > cap {
            resp.hits.split_off(cap)
        } else {
            Vec::new()
        };
        let rest_outline = if resp.outline.len() > cap {
            resp.outline.split_off(cap)
        } else {
            Vec::new()
        };
        let rest_rows = if resp.rows.len() > cap {
            resp.rows.split_off(cap)
        } else {
            Vec::new()
        };
        let had_rest = !rest_edges.is_empty()
            || !rest_hits.is_empty()
            || !rest_outline.is_empty()
            || !rest_rows.is_empty();
        if !had_rest {
            return;
        }
        resp.truncated = Some(true);
        let tail = QuerySurfaceResponse {
            schema_version: resp.schema_version,
            surface: resp.surface.clone(),
            coverage: resp.coverage.clone(),
            edges: rest_edges,
            hits: rest_hits,
            outline: rest_outline,
            rows: rest_rows,
            ..Default::default()
        };
        let page = super::page::page_document(
            "query_surface",
            serde_json::to_value(&tail).unwrap_or(serde_json::Value::Null),
        );
        if let Some(cursor) = super::page::spill_page(store_root, &page) {
            super::page::remember_session_cursor(session, &cursor);
            resp.next_cursor = Some(cursor);
        }
    }

    fn compact_visible_after_spill(resp: &mut QuerySurfaceResponse) {
        if resp.full_ref.is_none() {
            return;
        }
        let QuerySurfaceResponse {
            schema_version,
            surface,
            coverage,
            full_ref,
            decl_ref,
            symbol,
            skeleton,
            truncated,
            skeletons,
            delta,
            reading_set,
            reading_set_closure,
            capsule,
            absence_certificate,
            refs_footer,
            accounting,
            error,
            next_cursor,
            ..
        } = std::mem::take(resp);
        *resp = QuerySurfaceResponse {
            schema_version,
            surface,
            coverage,
            full_ref,
            decl_ref,
            symbol,
            skeleton,
            skeletons,
            delta,
            reading_set,
            reading_set_closure,
            capsule,
            absence_certificate,
            refs_footer,
            accounting,
            error,
            truncated: truncated.or(Some(true)),
            next_cursor,
            ..Default::default()
        };
    }

    fn apply_ref_first_budget_policy(
        resp: &mut QuerySurfaceResponse,
        store_root: Option<&std::path::Path>,
    ) {
        Self::scrub_ref_first_hit_metadata(&mut resp.hits);
        if resp.surface == "recall" && !resp.rows.is_empty() {
            resp.capsule = None;
            return;
        }
        if Self::response_has_detail(resp) {
            resp.capsule = None;
            Self::spill_ref_first_detail(resp, store_root);
            Self::compact_visible_after_spill(resp);
            return;
        }
        if resp.capsule.is_some() {
            Self::spill_or_keep_capsule(resp, store_root);
        } else {
            Self::spill_empty_shell_if_needed(resp, store_root);
        }
    }

    fn response_has_detail(resp: &QuerySurfaceResponse) -> bool {
        !resp.edges.is_empty()
            || !resp.hits.is_empty()
            || !resp.outline.is_empty()
            || !resp.rows.is_empty()
    }

    fn spill_or_keep_capsule(
        resp: &mut QuerySurfaceResponse,
        store_root: Option<&std::path::Path>,
    ) {
        let Some(capsule) = resp.capsule.take() else {
            return;
        };
        let spill = serde_json::to_string(&capsule).unwrap_or_default();
        if graphzero_store::store::query::tokens_for_str(&spill) <= 4 {
            resp.capsule = Some(capsule);
            return;
        }
        if let Some(id) = Self::spill_query_id(store_root, &spill) {
            resp.full_ref = Some(format!("gz://query/{id}"));
            resp.truncated = Some(true);
            Self::compact_visible_after_spill(resp);
        } else {
            // Keep capsule visible rather than advertising a ghost spill ref.
            resp.capsule = Some(capsule);
        }
    }

    fn spill_empty_shell_if_needed(
        resp: &mut QuerySurfaceResponse,
        store_root: Option<&std::path::Path>,
    ) {
        let Some(root) = store_root else {
            return;
        };
        if resp.decl_ref.is_some() || resp.full_ref.is_some() || resp.error.is_some() {
            return;
        }
        let spill = serde_json::to_string(&*resp).unwrap_or_default();
        let Some(id) = Self::spill_query_id(Some(root), &spill) else {
            return;
        };
        resp.full_ref = Some(format!("gz://query/{id}"));
        resp.truncated = Some(true);
        Self::compact_visible_after_spill(resp);
    }

    fn apply_budget_policy(
        resp: &mut QuerySurfaceResponse,
        budget: usize,
        store_root: Option<&std::path::Path>,
        session: Option<&str>,
    ) {
        if budget <= 1 {
            Self::apply_ref_first_budget_policy(resp, store_root);
        } else {
            Self::apply_detail_cap(resp, budget.min(32), store_root, session);
        }
    }

    fn serialized_token_estimate<T: Serialize>(value: &T) -> usize {
        struct ByteCounter {
            bytes: usize,
        }

        impl io::Write for ByteCounter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.bytes = self.bytes.saturating_add(buf.len());
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut counter = ByteCounter { bytes: 0 };
        serde_json::to_writer(&mut counter, value).expect("query surface response serializes");
        counter.bytes.div_ceil(4)
    }

    fn serialize_with_accounting(resp: &QuerySurfaceResponse, budget: usize) -> String {
        #[derive(Serialize)]
        struct BudgetAccounting {
            visible_tokens: usize,
            budget: usize,
        }

        #[derive(Serialize)]
        struct AccountedResponse<'a> {
            #[serde(flatten)]
            response: &'a QuerySurfaceResponse,
            accounting: BudgetAccounting,
        }

        let visible_tokens = Self::serialized_token_estimate(resp);
        serde_json::to_string(&AccountedResponse {
            response: resp,
            accounting: BudgetAccounting {
                visible_tokens,
                budget,
            },
        })
        .expect("query surface response serializes")
    }

    pub fn to_json_string_with_budget(
        resp: &QuerySurfaceResponse,
        budget: usize,
        store_root: Option<&std::path::Path>,
    ) -> String {
        let mut r = resp.clone();
        Self::apply_budget_policy(&mut r, budget, store_root, None);
        if budget <= 1
            && let Some(shell) = Self::compact_budget_one_shell(&r)
        {
            return shell;
        }
        Self::serialize_with_accounting(&r, budget)
    }

    /// Budgeted domain payload as `Value` without string→parse round-trip.
    ///
    /// Prefer this on the dispatcher path: budget>1 uses `to_value` once;
    /// budget-1 shells that are non-JSON become `{"raw": ...}` without a failed parse.
    /// Success budget-1 envelopes also carry additive `next` expand/capsule/export hints.
    pub fn to_json_value_with_budget(
        resp: &QuerySurfaceResponse,
        budget: usize,
        store_root: Option<&std::path::Path>,
    ) -> Value {
        Self::to_json_value_with_budget_and_session(resp, budget, store_root, None)
    }

    pub fn to_json_value_with_budget_and_session(
        resp: &QuerySurfaceResponse,
        budget: usize,
        store_root: Option<&std::path::Path>,
        session: Option<&str>,
    ) -> Value {
        let mut r = resp.clone();
        Self::apply_budget_policy(&mut r, budget, store_root, session);
        if budget <= 1
            && let Some(shell) = Self::compact_budget_one_shell(&r)
        {
            let next = if r.error.is_some() {
                Vec::new()
            } else {
                Self::budget_one_next_hints_for_response(&r, &shell)
            };
            return Self::wrap_budget_one_shell_value(&shell, next);
        }
        // Compact-shell envelopes only. Fallthrough (including fail-open
        // capsule dumps) must not advertise expand/capsule/export next-hints.
        Self::value_with_accounting(&r, budget)
    }

    /// Resume a spilled query-surface page. `None` if the cursor is missing or not a page.
    pub fn resume_query_cursor(
        store_root: &std::path::Path,
        cursor: &str,
        budget: usize,
        session: Option<&str>,
    ) -> Option<Value> {
        let page = super::page::load_page(store_root, cursor)?;
        let payload = super::page::payload_if_kind(&page, "query_surface")?;
        let resp: QuerySurfaceResponse = serde_json::from_value(payload).ok()?;
        Some(Self::to_json_value_with_budget_and_session(
            &resp,
            budget,
            Some(store_root),
            session,
        ))
    }

    /// CLI verb fragment for budget-1 next-step hints (`orient --surface X` / `search`).
    fn budget_one_cli_verb(surface: &str) -> String {
        match surface {
            "search" => "search".to_string(),
            other if other.is_empty() => "orient --surface symbol".to_string(),
            other => format!("orient --surface {other}"),
        }
    }

    /// Prefer full_ref, then decl_ref, then a ref-shaped raw shell.
    fn budget_one_expand_ref(resp: &QuerySurfaceResponse, shell: &str) -> Option<String> {
        if let Some(r) = resp
            .full_ref
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(r.to_string());
        }
        if let Some(r) = resp
            .decl_ref
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(r.to_string());
        }
        let trimmed = shell.trim();
        if trimmed.starts_with("q:") || trimmed.starts_with("g:") || trimmed.starts_with("gz://") {
            return Some(trimmed.to_string());
        }
        None
    }

    fn budget_one_next_hints(verb: &str, reference: Option<&str>) -> Vec<String> {
        let expand_target = reference.unwrap_or("<ref>");
        vec![
            format!("graphzero expand {expand_target}"),
            format!("graphzero {verb} ... --format capsule"),
            format!("graphzero {verb} ... --export PATH"),
        ]
    }

    fn budget_one_next_hints_for_response(resp: &QuerySurfaceResponse, shell: &str) -> Vec<String> {
        if resp.error.is_some() {
            return Vec::new();
        }
        let verb = Self::budget_one_cli_verb(&resp.surface);
        let reference = Self::budget_one_expand_ref(resp, shell);
        Self::budget_one_next_hints(&verb, reference.as_deref())
    }

    /// Wrap a budget-1 shell string as JSON Value, attaching additive `next` hints.
    ///
    /// Non-JSON shells become `{"raw": ...}`. JSON objects keep existing keys and gain `next`.
    fn wrap_budget_one_shell_value(shell: &str, next: Vec<String>) -> Value {
        match serde_json::from_str::<Value>(shell) {
            Ok(Value::Object(mut map)) => {
                if !next.is_empty() {
                    map.insert("next".into(), serde_json::json!(next));
                }
                Value::Object(map)
            }
            Ok(other) => other,
            Err(_) => {
                if next.is_empty() {
                    serde_json::json!({ "raw": shell })
                } else {
                    serde_json::json!({ "raw": shell, "next": next })
                }
            }
        }
    }

    fn value_with_accounting(resp: &QuerySurfaceResponse, budget: usize) -> Value {
        #[derive(Serialize)]
        struct BudgetAccounting {
            visible_tokens: usize,
            budget: usize,
        }

        #[derive(Serialize)]
        struct AccountedResponse<'a> {
            #[serde(flatten)]
            response: &'a QuerySurfaceResponse,
            accounting: BudgetAccounting,
        }

        let visible_tokens = Self::serialized_token_estimate(resp);
        serde_json::to_value(&AccountedResponse {
            response: resp,
            accounting: BudgetAccounting {
                visible_tokens,
                budget,
            },
        })
        .expect("query surface response serializes")
    }

    fn compact_budget_one_shell(resp: &QuerySurfaceResponse) -> Option<String> {
        if resp.surface == "delta"
            && let Some(ref d) = resp.delta
        {
            let comp = super::delta::DeltaComputation {
                since: d.since.clone(),
                changed: d.changed.clone(),
                added: d.added.clone(),
                removed: d.removed.clone(),
                unchanged_count: d.unchanged_count,
            };
            return Some(super::delta::format_delta_budget_one(
                &comp,
                &resp.skeletons,
            ));
        }
        if resp.surface == "outline" && !resp.skeleton.is_empty() {
            return Some(resp.skeleton.clone());
        }
        if resp.surface == "recall" {
            let target = resp.symbol.as_deref().unwrap_or("?");
            if resp.rows.is_empty() {
                return Some(format!("mem: 0 facts for {target}"));
            }
            let n = resp.rows.len();
            let mut lines = vec![format!("mem: {n} facts for {target}")];
            for row in resp.rows.iter().take(2) {
                let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("note");
                let text = row.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let preview: String = text.chars().take(80).collect();
                lines.push(format!("  {kind}: {preview} (gz://mem/{id})"));
            }
            return Some(lines.join("\n"));
        }
        if let Some(id) = Self::query_id_from_ref(resp.full_ref.as_deref())
            .or_else(|| Self::query_id_from_ref(resp.decl_ref.as_deref()))
        {
            return Some(graphzero_store::store::query::query_shell(&id));
        }
        if let Some(decl) = &resp.decl_ref
            && decl.starts_with("g:")
        {
            return Some(decl.clone());
        }
        None
    }

    fn query_id_from_ref(reference: Option<&str>) -> Option<String> {
        let reference = reference?;
        let id = reference
            .strip_prefix("gz://query/")
            .or_else(|| reference.strip_prefix("gz://q/"))?;
        Some(id.to_string())
    }
}
