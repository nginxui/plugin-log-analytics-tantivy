//! The tantivy index on disk and the reader over it.

use std::path::{Path, PathBuf};

use tantivy::indexer::{IndexWriterOptions, LogMergePolicy};
use tantivy::store::{Compressor, ZstdCompressor};
use tantivy::{Index, IndexReader, IndexSettings, IndexWriter, ReloadPolicy, Searcher, TantivyDocument};

use crate::schema::{self, Fields};
use crate::sizing::Sizing;
use crate::state::IndexState;
use crate::tokenizer;

/// Merge policy of the log index, tuned for a bulk import followed by small
/// appends: segments merge in layers from 100 000 documents up to two
/// million, and a segment with a third of its documents deleted is merged.
const MERGE_MIN_SEGMENTS: usize = 8;
const MERGE_MAX_DOCS: usize = 2_000_000;
const MERGE_MIN_LAYER: u32 = 100_000;
const MERGE_DELETE_RATIO: f32 = 0.3;

/// Compression level of the stored lines.
const DOCSTORE_ZSTD_LEVEL: i32 = 3;

/// Settings of a new index: the stored lines are compressed with zstd.
fn index_settings() -> IndexSettings {
    IndexSettings {
        docstore_compression: Compressor::Zstd(ZstdCompressor { compression_level: Some(DOCSTORE_ZSTD_LEVEL) }),
        ..Default::default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("index: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    #[error("index directory: {0}")]
    Io(#[from] std::io::Error),
}

/// An open index with its reader.
pub struct Store {
    dir: PathBuf,
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

fn schema_text(schema: &tantivy::schema::Schema) -> Option<String> {
    serde_json::to_string(schema).ok()
}

impl Store {
    /// Opens the index in `dir`, or creates it. An index that cannot be opened,
    /// that has another schema or another format is replaced by an empty one,
    /// its files are read again from the logs.
    pub fn open(dir: &Path) -> Result<(Store, IndexState), StoreError> {
        std::fs::create_dir_all(dir)?;
        let (schema, fields) = schema::build();

        if dir.join("meta.json").exists() {
            if let Ok(index) = Index::open_in_dir(dir) {
                tokenizer::register(&index);
                let same_schema = schema_text(&index.schema()) == schema_text(&schema);
                let same_settings = index.settings().docstore_compression == index_settings().docstore_compression;
                let state = index.load_metas().ok().and_then(|m| IndexState::from_payload(m.payload.as_deref()));
                if let (true, true, Some(state)) = (same_schema, same_settings, state) {
                    let reader = Self::make_reader(&index)?;
                    return Ok((Store { dir: dir.to_path_buf(), index, reader, fields }, state));
                }
            }
            Self::clear(dir)?;
        }

        let (index, reader) = Self::create(dir)?;
        Ok((Store { dir: dir.to_path_buf(), index, reader, fields }, IndexState::default()))
    }

    fn create(dir: &Path) -> Result<(Index, IndexReader), StoreError> {
        let (schema, _) = schema::build();
        let index = Index::builder().schema(schema).settings(index_settings()).create_in_dir(dir)?;
        tokenizer::register(&index);
        let reader = Self::make_reader(&index)?;
        Ok((index, reader))
    }

    fn make_reader(index: &Index) -> tantivy::Result<IndexReader> {
        index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()
    }

    fn clear(dir: &Path) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn fields(&self) -> &Fields {
        &self.fields
    }

    /// A snapshot of the committed documents.
    pub fn searcher(&self) -> Searcher {
        self.reader.searcher()
    }

    /// Makes the last commit visible to new searchers. It runs after every
    /// commit, the reader never reloads by itself.
    pub fn reload(&self) {
        if let Err(e) = self.reader.reload() {
            nginxui_plugin_sdk::warn!("could not reload the index reader: {e}");
        }
    }

    /// A writer with the merge policy of the log index.
    pub fn writer(&self, sizing: &Sizing) -> Result<IndexWriter<TantivyDocument>, StoreError> {
        let options = IndexWriterOptions::builder()
            .num_worker_threads(sizing.threads)
            .memory_budget_per_thread((sizing.heap_mb << 20) / sizing.threads)
            .num_merge_threads(sizing.merge_threads)
            .build();
        let writer = self.index.writer_with_options::<TantivyDocument>(options)?;
        let mut policy = LogMergePolicy::default();
        policy.set_min_num_segments(MERGE_MIN_SEGMENTS);
        policy.set_max_docs_before_merge(MERGE_MAX_DOCS);
        policy.set_min_layer_size(MERGE_MIN_LAYER);
        policy.set_del_docs_ratio_before_merge(MERGE_DELETE_RATIO);
        writer.set_merge_policy(Box::new(policy));
        Ok(writer)
    }

    /// Merges the small segments a bulk import leaves behind: each writer
    /// thread ends with a segment of its own, and the merge policy leaves
    /// them apart while they are few. Segments under half the size of the
    /// largest are merged into one when there are two or more. Returns how
    /// many were merged.
    pub fn merge_tail(&self) -> Result<usize, StoreError> {
        let metas = self.index.searchable_segment_metas()?;
        let largest = metas.iter().map(|m| m.num_docs()).max().unwrap_or(0);
        let small: Vec<_> = metas.iter().filter(|m| m.num_docs() < largest / 2).map(|m| m.id()).collect();
        if small.len() < 2 {
            return Ok(0);
        }
        // The merge needs no indexing heap, the smallest writer does
        let options = IndexWriterOptions::builder().num_worker_threads(1).num_merge_threads(1).build();
        let mut writer = self.index.writer_with_options::<TantivyDocument>(options)?;
        writer.set_merge_policy(Box::new(tantivy::indexer::NoMergePolicy));
        writer.merge(&small).wait()?;
        writer.wait_merging_threads()?;
        Ok(small.len())
    }

    /// Bytes the index files take on disk.
    pub fn disk_size(&self) -> u64 {
        std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter_map(|e| e.metadata().ok())
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_reopens_and_replaces_a_foreign_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index");
        {
            let (store, state) = Store::open(&path).unwrap();
            assert_eq!(state, IndexState::default());
            let mut writer = store.writer(&crate::sizing::sizing(Some(1 << 29), 1)).unwrap();
            let mut s = IndexState::default();
            s.groups.insert("g".into(), Default::default());
            let mut prepared = writer.prepare_commit().unwrap();
            prepared.set_payload(&s.to_payload());
            prepared.commit().unwrap();
        }
        let (_store, state) = Store::open(&path).unwrap();
        assert!(state.groups.contains_key("g"));

        // A payload of another format empties the index
        {
            let (store, _) = Store::open(&path).unwrap();
            let mut writer = store.writer(&crate::sizing::sizing(Some(1 << 29), 1)).unwrap();
            let foreign = IndexState { format: 999, ..Default::default() };
            let mut prepared = writer.prepare_commit().unwrap();
            prepared.set_payload(&foreign.to_payload());
            prepared.commit().unwrap();
        }
        let (_store, state) = Store::open(&path).unwrap();
        assert_eq!(state, IndexState::default());
    }
    #[test]
    fn the_small_segments_of_an_import_are_merged_and_the_state_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(&dir.path().join("index")).unwrap();
        let ts = store.fields().ts;
        let commit = |docs: i64, state: &IndexState| {
            let mut writer = store.writer(&crate::sizing::sizing(Some(1 << 29), 1)).unwrap();
            writer.set_merge_policy(Box::new(tantivy::indexer::NoMergePolicy));
            for i in 0..docs {
                writer.add_document(tantivy::doc!(ts => i)).unwrap();
            }
            let mut prepared = writer.prepare_commit().unwrap();
            prepared.set_payload(&state.to_payload());
            prepared.commit().unwrap();
            writer.wait_merging_threads().unwrap();
        };
        let mut state = IndexState::default();
        state.groups.insert("g".into(), Default::default());
        commit(1000, &state);
        commit(100, &state);
        commit(100, &state);
        commit(100, &state);

        assert_eq!(store.merge_tail().unwrap(), 3);
        let metas = store.index.searchable_segment_metas().unwrap();
        let mut sizes: Vec<u32> = metas.iter().map(|m| m.num_docs()).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, [300, 1000]);
        assert_eq!(store.merge_tail().unwrap(), 0);
        drop(store);
        let (_, reopened) = Store::open(&dir.path().join("index")).unwrap();
        assert!(reopened.groups.contains_key("g"), "the merge keeps the state of the last commit");
    }
}
