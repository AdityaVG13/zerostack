//! Journal bookmark surface (fszero-xdxh).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalBookmark {
    pub head_seq: i64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct JournalBookmarks {
    head_seq: i64,
    named: std::collections::BTreeMap<String, i64>,
}

impl JournalBookmarks {
    pub fn set_head(&mut self, seq: i64) {
        self.head_seq = seq;
    }

    pub fn head(&self) -> JournalBookmark {
        JournalBookmark {
            head_seq: self.head_seq,
            label: None,
        }
    }

    pub fn bookmark(&mut self, label: impl Into<String>, seq: i64) {
        self.named.insert(label.into(), seq);
    }

    pub fn since(&self, label: &str) -> Option<i64> {
        self.named.get(label).copied()
    }

    pub fn mutations_since(&self, after_seq: i64, all: &[(i64, String)]) -> Vec<(i64, String)> {
        all.iter()
            .filter(|(seq, _)| *seq > after_seq)
            .cloned()
            .collect()
    }
}
