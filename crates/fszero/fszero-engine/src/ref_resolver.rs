use super::*;

impl FSZeroSession {
    /// Single resolution funnel behind `session.expand`, the `X` op
    /// (CLI/MCP `fszero.expand`) and the CodeMode `fs.expand` connector.
    /// Named keys ("last", "search", view ids/aliases) resolve session-side;
    /// everything ref-shaped funnels into
    /// `RecoveryStore::expand_with_tiers`, where canonical ZeroRef v1 blob
    /// refs — `(fz|gz|tz)://blob/<sha256>[#B|#L]` — take the strict typed
    /// path (`expand_zeroref`) and legacy keys take the compatibility path.
    /// All surfaces therefore return identical bytes/error classes for the
    /// same ref (fszero-c6q.2).
    pub fn resolve_ref_payload_detailed(&self, key: &str) -> Result<Vec<u8>, String> {
        if key.is_empty() {
            return Err(super::recovery::ref_not_found_err("<empty>"));
        }
        if key == "last" || key == "lastR" {
            return self
                .resolve_last_payload()
                .ok_or_else(|| super::recovery::ref_not_found_err(key));
        }
        if key.starts_with("fz://seq/")
            || key.starts_with("tz://seq/")
            || key.starts_with("gz://seq/")
        {
            return Err(super::recovery::seq_ref_scoped_err(key));
        }
        if key.starts_with("fz://")
            || key.starts_with("tz://")
            || key.starts_with("gz://")
            || key.starts_with("view_")
            || key.contains('/')
        {
            if let Some(bytes) = self.expand_read_view(key) {
                return Ok(bytes);
            }
            return self.expand_with_tiers_via_store_map(key);
        }
        if let Ok(id) = key.parse::<u32>() {
            return self
                .resolve_view_id_payload(id)
                .ok_or_else(|| super::recovery::ref_not_found_err(key));
        }
        if let Some(bytes) = self.expand_read_view(key) {
            return Ok(bytes);
        }
        self.expand_with_tiers_via_store_map(key)
    }

    /// Expand with RecoveryStore tiers, then other session-mapped stores.
    pub fn expand_with_tiers_via_store_map(&self, key: &str) -> Result<Vec<u8>, String> {
        match self.recovery.expand_with_tiers(key) {
            Ok(bytes) => Ok(bytes),
            Err(err) => self
                .with_other_mapped_stores(|remote| remote.expand_with_tiers(key).ok())
                .ok_or(err),
        }
    }

    pub fn resolve_last_payload(&self) -> Option<Vec<u8>> {
        if self.views.last_view_id > 0 {
            if let Some(payload) = self.resolve_view_id_payload(self.views.last_view_id) {
                return Some(payload);
            }
        }
        if let Some(ref content_ref) = self.views.last_read_ref {
            if let Some(payload) = self.recovery.expand(content_ref) {
                return Some(payload);
            }
        }
        None
    }

    pub fn resolve_view_id_payload(&self, id: u32) -> Option<Vec<u8>> {
        self.expand_read_view(&format!("view_{id}/bytes"))
            .or_else(|| self.recovery.expand(&format!("view_{id}/bytes")))
            .or_else(|| {
                self.expand_read_view(&format!("view_{id}/ref"))
                    .or_else(|| self.recovery.expand(&format!("view_{id}/ref")))
                    .and_then(|r| self.recovery.expand(&String::from_utf8_lossy(&r)))
            })
            .or_else(|| self.expand_read_view(&format!("r{id}/bytes")))
            .or_else(|| self.recovery.expand(&format!("r{id}/bytes")))
    }

    pub fn resolve_view_for_edit(&self, id: u32) -> Option<(PathBuf, Vec<u8>)> {
        let bytes = self.resolve_view_id_payload(id)?;
        let path = self
            .expand_read_view(&format!("view_{id}/path"))
            .or_else(|| self.recovery.expand(&format!("view_{id}/path")))
            .or_else(|| self.expand_read_view(&format!("r{id}/path")))
            .or_else(|| self.recovery.expand(&format!("r{id}/path")))?;
        Some((
            PathBuf::from(String::from_utf8_lossy(&path).into_owned()),
            bytes,
        ))
    }

    pub fn invalidate_path_cache_entry(&mut self, path: &Path) {
        self.caches
            .paths
            .retain(|_, cached| cached.as_path() != path);
        self.caches.content.remove(path);
    }

    /// Drop ls parent entry + path/content caches after a successful tree write.
    pub fn invalidate_path_and_parent_ls(&mut self, path: &Path) {
        self.caches
            .ls
            .remove(&path.parent().unwrap_or(path).to_path_buf());
        self.invalidate_path_cache_entry(path);
    }

    /// After mutation or rollback restore/remove: drop path/content/ls caches and reindex.
    pub fn refresh_path_after_mutation(&mut self, path: &Path) {
        self.invalidate_path_and_parent_ls(path);
        self.reindex_path(path);
    }

    /// Clear search/compound result caches (index rebuild / single-file reindex).
    ///
    /// Full wipe is coarse invalidation (fszero-2uhi): each dropped entry is
    /// counted as a `coarse_wipe` cache miss so over-invalidation is measurable.
    pub fn clear_query_caches(&mut self) {
        let n = (self.caches.search.len() + self.caches.compound.len()) as u64;
        self.caches.search.clear();
        self.caches.compound.clear();
        // Single-file reindex keeps the current AST generation, so generation-
        // based invalidation alone cannot retire certified-empty search answers.
        self.caches.negative_cache.clear();
        self.views.last_search_payload = None;
        self.views.last_compound_payload = None;
        self.recovery.note_coarse_wipe_misses(n);
    }
}
