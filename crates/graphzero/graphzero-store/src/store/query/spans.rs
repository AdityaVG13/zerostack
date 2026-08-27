//! Span lookup helpers over sorted symbol span tables.

use super::super::format::SpanEntry;

/// Return the sub-slice of spans for `symbol_id`. Spans must be sorted by `symbol_id`.
pub fn span_range(spans: &[SpanEntry], id: u32) -> &[SpanEntry] {
    let lo = spans.partition_point(|s| s.symbol_id < id);
    let hi = spans.partition_point(|s| s.symbol_id <= id);
    &spans[lo..hi]
}
