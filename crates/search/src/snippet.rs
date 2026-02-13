//! Snippet extraction and score boosting.

use std::path::Path;

use crate::Snippet;

// ---------------------------------------------------------------------------
// Score boosting
// ---------------------------------------------------------------------------

pub(crate) fn apply_boost(path: &str, score: f32) -> f32 {
    let p = path.to_lowercase();

    // Tests: 0.5x
    if p.contains("/test") || p.contains("_test.") || p.contains(".test.") || p.contains(".spec.") {
        return score * 0.5;
    }

    // Mocks: 0.4x
    if p.contains("/mock") || p.contains(".mock.") {
        return score * 0.4;
    }

    // Docs: 0.6x
    if p.ends_with(".md") || p.contains("/docs/") {
        return score * 0.6;
    }

    // Source: 1.1x
    if p.contains("/src") || p.contains("/lib") {
        return score * 1.1;
    }

    score
}

// ---------------------------------------------------------------------------
// Query terms
// ---------------------------------------------------------------------------

pub(crate) fn extract_query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Snippet extraction
// ---------------------------------------------------------------------------

pub(crate) fn extract_snippets(
    file_path: &Path,
    query_terms: &[String],
    context: usize,
    max_snippets: usize,
) -> Vec<Snippet> {
    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() || query_terms.is_empty() {
        return vec![];
    }

    // Find matching line indices
    let mut match_indices = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();

        if query_terms.iter().any(|term| lower.contains(term)) {
            match_indices.push(i);
        }
    }

    if match_indices.is_empty() {
        return vec![];
    }

    // Build windows and merge overlapping ones
    let mut windows: Vec<(usize, usize)> = Vec::new();

    for &idx in &match_indices {
        let start = idx.saturating_sub(context);
        let end = (idx + context + 1).min(lines.len());

        if let Some(last) = windows.last_mut()
            && start <= last.1
        {
            last.1 = end;
            continue;
        }

        windows.push((start, end));
    }

    windows
        .into_iter()
        .take(max_snippets)
        .map(|(start, end)| Snippet {
            line_number: start + 1, // 1-based
            lines: lines[start..end].iter().map(|l| l.to_string()).collect(),
        })
        .collect()
}
