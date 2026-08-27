//! Cursor-store pagination for large search result sets (fszero-enuj).
//!
//! Opaque session-scoped cursor tokens; LRU-evicted at [`CURSOR_CAP`].

use std::collections::{BTreeMap, VecDeque};

/// Max live cursors per session (fff uses 20).
pub const CURSOR_CAP: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    pub items: Vec<String>,
    pub next_cursor: Option<String>,
    pub total_hint: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SearchCursorStore {
    /// cursor token -> remaining items
    pages: BTreeMap<String, Vec<String>>,
    /// LRU order: front = oldest (evict first).
    order: VecDeque<String>,
    next_id: u64,
}

impl SearchCursorStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    fn touch(&mut self, token: &str) {
        self.order.retain(|t| t != token);
        self.order.push_back(token.to_string());
    }

    fn evict_if_needed(&mut self) {
        while self.pages.len() > CURSOR_CAP {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            self.pages.remove(&old);
        }
    }

    /// Create first page; store remainder under a cursor token.
    pub fn page(&mut self, all: Vec<String>, page_size: usize) -> SearchPage {
        let total = all.len();
        if page_size == 0 || all.len() <= page_size {
            return SearchPage {
                items: all,
                next_cursor: None,
                total_hint: total,
            };
        }
        let mut rest = all;
        let items: Vec<String> = rest.drain(..page_size).collect();
        let token = format!("cur-{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.pages.insert(token.clone(), rest);
        self.touch(&token);
        self.evict_if_needed();
        // If we just evicted ourselves (cap 0 edge), still return items without cursor.
        if !self.pages.contains_key(&token) {
            return SearchPage {
                items,
                next_cursor: None,
                total_hint: total,
            };
        }
        SearchPage {
            items,
            next_cursor: Some(token),
            total_hint: total,
        }
    }

    pub fn next(&mut self, cursor: &str, page_size: usize) -> Option<SearchPage> {
        let rest = self.pages.remove(cursor)?;
        self.order.retain(|t| t != cursor);
        Some(self.page(rest, page_size))
    }

    /// Invalidate all cursors (e.g. index mutation).
    pub fn invalidate_all(&mut self) {
        self.pages.clear();
        self.order.clear();
    }
}
