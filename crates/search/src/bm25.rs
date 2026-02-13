//! BM25 full-text search powered by Tantivy (in-memory).

use anyhow::{Context, Result};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, STORED, STRING, Schema, TEXT, Value as _};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

// ---------------------------------------------------------------------------
// Bm25Index
// ---------------------------------------------------------------------------

pub(crate) struct Bm25Index {
    index: Index,
    path_field: Field,
    content_field: Field,
}

impl Bm25Index {
    pub fn new() -> Result<Self> {
        let mut schema_builder = Schema::builder();
        let path_field = schema_builder.add_text_field("path", STRING | STORED);
        let content_field = schema_builder.add_text_field("content", TEXT);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);

        Ok(Self {
            index,
            path_field,
            content_field,
        })
    }

    pub fn writer(&self) -> Result<IndexWriter> {
        self.index
            .writer(50_000_000)
            .context("failed to create index writer")
    }

    pub fn add(&self, writer: &mut IndexWriter, path: &str, content: &str) {
        let mut doc = TantivyDocument::new();
        doc.add_text(self.path_field, path);
        doc.add_text(self.content_field, content);
        let _ = writer.add_document(doc);
    }

    pub fn remove(&self, writer: &mut IndexWriter, path: &str) {
        writer.delete_term(Term::from_field_text(self.path_field, path));
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>> {
        let reader = self.index.reader().context("failed to open reader")?;
        let searcher = reader.searcher();

        let query_parser = QueryParser::for_index(&self.index, vec![self.content_field]);

        let parsed_query = query_parser
            .parse_query(query)
            .map_err(|e| anyhow::anyhow!("query parse error: {e}"))?;

        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit))
            .context("search failed")?;

        let mut results = Vec::new();

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .context("failed to retrieve document")?;

            let path = doc
                .get_first(self.path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            results.push((path, score));
        }

        Ok(results)
    }
}
