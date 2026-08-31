//! Blast-radius confidence: min(edge confidence) × coverage factor (tier-A). Objective (honest
//! claim): `score = min(provenance-path edge confidences) × (tier_a_pct / 100)`.

/// Combine edge confidences with tier-A coverage percent (0–100).
/// Computes `min(edges) × (tier_a_pct / 100)`.
/// `edge_confidences` is the provenance path (path-min), never a singleton best-edge.
pub fn blast_finding_score(edge_confidences: &[f64], tier_a_pct: f64) -> f64 {
    if edge_confidences.is_empty() {
        return 0.0;
    }
    let min_edge = edge_confidences
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    if !min_edge.is_finite() {
        return 0.0;
    }
    let coverage_factor = (tier_a_pct / 100.0).clamp(0.0, 1.0);
    min_edge * coverage_factor
}
