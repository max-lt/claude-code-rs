//! Hybrid search: BM25 + semantic (fastembed) with Reciprocal Rank Fusion.
//!
//! Session-scoped, in-memory index with incremental mtime-based updates.
//! Embeddings are computed lazily on the first `search()` call.

mod bm25;
mod hybrid;
mod semantic;
mod snippet;
pub(crate) mod walk;

use std::path::Path;

use anyhow::{Context, Result};

use bm25::Bm25Index;
use semantic::SemanticIndex;
use snippet::{apply_boost, extract_query_terms, extract_snippets};
use walk::FileWalker;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct SearchIndex {
    bm25: Bm25Index,
    semantic: SemanticIndex,
    walker: FileWalker,
}

pub struct OpenStats {
    pub files: usize,
    pub bytes: u64,
}

pub struct UpdateStats {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
}

impl UpdateStats {
    pub fn has_changes(&self) -> bool {
        self.added > 0 || self.modified > 0 || self.removed > 0
    }
}

pub struct SearchHit {
    pub path: String,
    pub score: f32,
    pub snippets: Vec<Snippet>,
}

#[derive(Debug, Clone)]
pub struct Snippet {
    pub line_number: usize,
    pub lines: Vec<String>,
}

// ---------------------------------------------------------------------------
// SearchIndex
// ---------------------------------------------------------------------------

impl SearchIndex {
    /// Build a new index by walking all files under `dir`.
    ///
    /// BM25 index is built immediately. Embeddings are deferred until the
    /// first `search()` call.
    pub fn open(dir: &Path) -> Result<(Self, OpenStats)> {
        let root_dir = dir
            .canonicalize()
            .with_context(|| format!("cannot resolve path: {}", dir.display()))?;

        let bm25 = Bm25Index::new()?;
        let semantic = SemanticIndex::new();
        let mut walker = FileWalker::new(root_dir);

        let (entries, walk_stats) = walker.walk_all()?;

        // Populate BM25 index
        let mut writer = bm25.writer()?;

        for entry in &entries {
            bm25.add(&mut writer, &entry.relative, &entry.content);
        }

        writer.commit().context("failed to commit BM25 index")?;

        let stats = OpenStats {
            files: walk_stats.files,
            bytes: walk_stats.bytes,
        };

        let index = Self {
            bm25,
            semantic,
            walker,
        };

        Ok((index, stats))
    }

    /// Incrementally update: diff mtimes, re-index changed files.
    pub fn update(&mut self) -> Result<UpdateStats> {
        let result = self.walker.walk_incremental()?;

        let stats = UpdateStats {
            added: result
                .changes
                .iter()
                .filter(|c| c.kind == walk::ChangeKind::Added)
                .count(),
            modified: result
                .changes
                .iter()
                .filter(|c| c.kind == walk::ChangeKind::Modified)
                .count(),
            removed: result.removed.len(),
        };

        if !stats.has_changes() {
            return Ok(stats);
        }

        // Update BM25 index
        let mut writer = self.bm25.writer()?;

        for change in &result.changes {
            if change.kind == walk::ChangeKind::Modified {
                self.bm25.remove(&mut writer, &change.relative);
            }

            self.bm25
                .add(&mut writer, &change.relative, &change.content);
        }

        for removed_path in &result.removed {
            self.bm25.remove(&mut writer, removed_path);
        }

        writer.commit().context("failed to commit BM25 update")?;

        // Update semantic index if it was already built
        if self.semantic.is_ready() {
            self.semantic
                .embed_incremental(&result.changes, &result.removed)?;
        }

        Ok(stats)
    }

    /// Hybrid search: BM25 + semantic via RRF, with score boosting and snippets.
    ///
    /// The first call triggers lazy embedding model load + batch embed of all files.
    pub fn search(
        &mut self,
        query: &str,
        limit: usize,
        context_lines: usize,
    ) -> Result<Vec<SearchHit>> {
        // Ensure semantic index is ready (lazy init)
        if !self.semantic.is_ready() {
            self.build_embeddings()?;
        }

        let fetch_limit = limit * 2;

        // BM25 search
        let bm25_results = self.bm25.search(query, fetch_limit)?;

        // Semantic search
        let semantic_results = self.semantic.search(query, fetch_limit)?;

        // RRF merge
        let merged = hybrid::rrf_merge(&bm25_results, &semantic_results, limit);

        // Build hits with boosting
        let mut hits: Vec<SearchHit> = merged
            .into_iter()
            .map(|(path, score)| {
                let boosted = apply_boost(&path, score);
                SearchHit {
                    path,
                    score: boosted,
                    snippets: vec![],
                }
            })
            .collect();

        // Re-sort by boosted score
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Extract snippets
        if context_lines > 0 {
            let query_terms = extract_query_terms(query);
            let root = self.walker.root();

            for hit in &mut hits {
                let full_path = root.join(&hit.path);
                hit.snippets = extract_snippets(&full_path, &query_terms, context_lines, 3);
            }
        }

        Ok(hits)
    }

    /// Walk all indexed files and batch-embed them.
    fn build_embeddings(&mut self) -> Result<()> {
        let (entries, _) = self.walker.walk_all()?;

        let files: Vec<(String, String)> = entries
            .into_iter()
            .map(|e| (e.relative, e.content))
            .collect();

        self.semantic.embed_all(&files)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();

        fs::create_dir_all(dir.path().join("src")).unwrap();

        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"hello world\");\n}\n",
        )
        .unwrap();

        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\
             pub fn error_handler() {\n    eprintln!(\"error handling logic\");\n}\n",
        )
        .unwrap();

        fs::write(
            dir.path().join("README.md"),
            "# Test Project\n\nThis is a test project with error handling.\n",
        )
        .unwrap();

        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_open_index() {
        let dir = setup_test_dir();
        let (_, stats) = SearchIndex::open(dir.path()).unwrap();

        assert!(
            stats.files >= 3,
            "expected at least 3 files, got {}",
            stats.files
        );
        assert!(stats.bytes > 0);
    }

    #[test]
    fn test_update_no_changes() {
        let dir = setup_test_dir();
        let (mut index, _) = SearchIndex::open(dir.path()).unwrap();

        let stats = index.update().unwrap();
        assert!(!stats.has_changes());
    }

    #[test]
    fn test_update_detects_added_file() {
        let dir = setup_test_dir();
        let (mut index, _) = SearchIndex::open(dir.path()).unwrap();

        // Add a new file
        fs::write(dir.path().join("src/new.rs"), "fn new_func() {}\n").unwrap();

        let stats = index.update().unwrap();
        assert_eq!(stats.added, 1);
        assert_eq!(stats.modified, 0);
        assert_eq!(stats.removed, 0);
    }

    #[test]
    fn test_update_detects_removed_file() {
        let dir = setup_test_dir();
        let (mut index, _) = SearchIndex::open(dir.path()).unwrap();

        // Remove a file
        fs::remove_file(dir.path().join("README.md")).unwrap();

        let stats = index.update().unwrap();
        assert_eq!(stats.removed, 1);
    }

    #[test]
    fn test_update_detects_modified_file() {
        let dir = setup_test_dir();
        let (mut index, _) = SearchIndex::open(dir.path()).unwrap();

        // Wait a tiny bit to ensure mtime changes
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Modify a file
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"modified content\");\n}\n",
        )
        .unwrap();

        let stats = index.update().unwrap();
        assert_eq!(stats.modified, 1);
    }

    #[test]
    fn test_bm25_search() {
        let dir = setup_test_dir();
        let (index, _) = SearchIndex::open(dir.path()).unwrap();

        // BM25-only search (bypass semantic by testing bm25 directly)
        let hits = index.bm25.search("hello world", 10).unwrap();
        assert!(!hits.is_empty(), "expected BM25 results for 'hello world'");
        assert!(hits[0].0.contains("main.rs"));
    }

    #[test]
    fn test_bm25_no_results() {
        let dir = setup_test_dir();
        let (index, _) = SearchIndex::open(dir.path()).unwrap();

        let hits = index.bm25.search("xyznonexistent", 10).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_boost_source_files() {
        let score = snippet::apply_boost("src/lib.rs", 1.0);
        assert!((score - 1.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_boost_test_files() {
        let score = snippet::apply_boost("tests/test_search.rs", 1.0);
        assert!((score - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_boost_doc_files() {
        let score = snippet::apply_boost("README.md", 1.0);
        assert!((score - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn test_boost_mock_files() {
        let score = snippet::apply_boost("src/mock/handler.rs", 1.0);
        assert!((score - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn test_is_text_file() {
        assert!(walk::is_text_file(Path::new("main.rs")));
        assert!(walk::is_text_file(Path::new("index.ts")));
        assert!(walk::is_text_file(Path::new("Dockerfile")));
        assert!(walk::is_text_file(Path::new("Makefile")));
        assert!(!walk::is_text_file(Path::new("image.png")));
        assert!(!walk::is_text_file(Path::new("binary.exe")));
    }

    #[test]
    fn test_is_binary() {
        assert!(!walk::is_binary(b"hello world"));
        assert!(walk::is_binary(b"hello\x00world"));
    }

    #[test]
    fn test_extract_query_terms() {
        let terms = snippet::extract_query_terms("error handling in Rust");
        assert_eq!(terms, vec!["error", "handling", "in", "rust"]);
    }

    #[test]
    fn test_extract_query_terms_filters_short() {
        let terms = snippet::extract_query_terms("a is ok");
        assert_eq!(terms, vec!["is", "ok"]);
    }
}
