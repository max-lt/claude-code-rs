//! Reciprocal Rank Fusion (RRF) for combining BM25 + semantic results.

use std::collections::HashMap;

const K: f32 = 60.0;

/// Merge BM25 and semantic results using RRF.
///
/// Each result set contributes `1 / (k + rank + 1)` per entry.
/// The merged list is sorted by combined RRF score, descending.
pub(crate) fn rrf_merge(
    bm25: &[(String, f32)],
    semantic: &[(String, f32)],
    limit: usize,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<&str, f32> = HashMap::new();

    for (rank, (path, _)) in bm25.iter().enumerate() {
        *scores.entry(path.as_str()).or_default() += 1.0 / (K + rank as f32 + 1.0);
    }

    for (rank, (path, _)) in semantic.iter().enumerate() {
        *scores.entry(path.as_str()).or_default() += 1.0 / (K + rank as f32 + 1.0);
    }

    let mut results: Vec<(String, f32)> = scores
        .into_iter()
        .map(|(path, score)| (path.to_string(), score))
        .collect();

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);

    results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rrf_merge_both_sources() {
        let bm25 = vec![
            ("a.rs".to_string(), 10.0),
            ("b.rs".to_string(), 5.0),
            ("c.rs".to_string(), 1.0),
        ];

        let semantic = vec![
            ("b.rs".to_string(), 0.9),
            ("d.rs".to_string(), 0.8),
            ("a.rs".to_string(), 0.7),
        ];

        let merged = rrf_merge(&bm25, &semantic, 10);

        // a.rs and b.rs appear in both → should have highest scores
        assert!(merged.len() >= 2);
        let top_paths: Vec<&str> = merged.iter().take(2).map(|(p, _)| p.as_str()).collect();
        assert!(top_paths.contains(&"a.rs"));
        assert!(top_paths.contains(&"b.rs"));
    }

    #[test]
    fn test_rrf_merge_limit() {
        let bm25 = vec![("a.rs".to_string(), 10.0), ("b.rs".to_string(), 5.0)];

        let semantic = vec![("c.rs".to_string(), 0.9), ("d.rs".to_string(), 0.8)];

        let merged = rrf_merge(&bm25, &semantic, 2);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_rrf_merge_empty() {
        let merged = rrf_merge(&[], &[], 10);
        assert!(merged.is_empty());
    }
}
