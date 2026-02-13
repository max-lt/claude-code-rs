//! Semantic search using fastembed (AllMiniLML6V2, 384-dim).
//!
//! The ONNX model is downloaded to the system cache on first use.
//! Embeddings are computed lazily on the first `search()` call.

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::walk::FileChange;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

struct EmbeddingEntry {
    path: String,
    vector: Vec<f32>,
}

// ---------------------------------------------------------------------------
// SemanticIndex
// ---------------------------------------------------------------------------

pub(crate) struct SemanticIndex {
    model: Option<TextEmbedding>,
    entries: Vec<EmbeddingEntry>,
}

impl SemanticIndex {
    pub fn new() -> Self {
        Self {
            model: None,
            entries: Vec::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Embed all files from scratch.
    pub fn embed_all(&mut self, files: &[(String, String)]) -> Result<()> {
        if files.is_empty() {
            self.entries.clear();
            return Ok(());
        }

        let model = self.ensure_model()?;

        let texts: Vec<String> = files
            .iter()
            .map(|(_, content)| truncate(content, 8192))
            .collect();

        let vectors = model
            .embed(texts, None)
            .context("failed to compute embeddings")?;

        self.entries.clear();

        for ((path, _), vector) in files.iter().zip(vectors) {
            self.entries.push(EmbeddingEntry {
                path: path.clone(),
                vector,
            });
        }

        Ok(())
    }

    /// Incrementally update embeddings for changed/removed files.
    pub fn embed_incremental(&mut self, changes: &[FileChange], removed: &[String]) -> Result<()> {
        if changes.is_empty() && removed.is_empty() {
            return Ok(());
        }

        // Remove entries for changed + removed files
        let to_remove: std::collections::HashSet<&str> = changes
            .iter()
            .map(|c| c.relative.as_str())
            .chain(removed.iter().map(|s| s.as_str()))
            .collect();

        self.entries
            .retain(|e| !to_remove.contains(e.path.as_str()));

        // Embed new/modified files
        if !changes.is_empty() {
            let model = self.ensure_model()?;

            let texts: Vec<String> = changes.iter().map(|c| truncate(&c.content, 8192)).collect();

            let vectors = model
                .embed(texts, None)
                .context("failed to compute embeddings")?;

            for (change, vector) in changes.iter().zip(vectors) {
                self.entries.push(EmbeddingEntry {
                    path: change.relative.clone(),
                    vector,
                });
            }
        }

        Ok(())
    }

    /// Search by cosine similarity. Returns (path, score) pairs.
    pub fn search(&mut self, query: &str, limit: usize) -> Result<Vec<(String, f32)>> {
        if self.entries.is_empty() {
            return Ok(vec![]);
        }

        let model = self.ensure_model()?;

        let query_vectors = model
            .embed(vec![query.to_string()], None)
            .context("failed to embed query")?;
        let query_vec = &query_vectors[0];

        let mut scored: Vec<(String, f32)> = self
            .entries
            .iter()
            .map(|e| (e.path.clone(), cosine_similarity(query_vec, &e.vector)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored)
    }

    fn ensure_model(&mut self) -> Result<&mut TextEmbedding> {
        if self.model.is_none() {
            let cache_dir = dirs::cache_dir()
                .context("could not find system cache directory")?
                .join("ccrs")
                .join("models");

            std::fs::create_dir_all(&cache_dir)
                .context("failed to create model cache directory")?;

            let mut options = InitOptions::default();
            options.model_name = EmbeddingModel::AllMiniLML6V2;
            options.cache_dir = cache_dir;
            options.show_download_progress = true;

            let model =
                TextEmbedding::try_new(options).context("failed to load embedding model")?;
            self.model = Some(model);
        }

        Ok(self.model.as_mut().unwrap())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}
