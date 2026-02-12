// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use blake3::Hasher;
use rmpv::Value;

use crate::blob_store::BlobStore;
use crate::cql::{self, CqlError, CqlQuery, IndexStats, SecondaryIndexes};
use crate::error::{Result, StoreError};
use crate::fs_store::{FsRootsIndex, TreeEntry};
use crate::turn_store::{ContextHead, TurnMeta, TurnRecord, TurnStore};

#[derive(Debug, Clone)]
pub struct TurnWithMeta {
    pub record: TurnRecord,
    pub meta: TurnMeta,
    pub payload: Option<Vec<u8>>,
}

/// Provenance captures the origin story of a context.
/// Extracted from the first turn's payload.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Provenance {
    // Context Lineage
    pub parent_context_id: Option<u64>,
    pub spawn_reason: Option<String>,
    pub root_context_id: Option<u64>,

    // Request Identity
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub correlation_id: Option<String>,

    // User Identity (on whose behalf)
    pub on_behalf_of: Option<String>,
    pub on_behalf_of_source: Option<String>,
    pub on_behalf_of_email: Option<String>,

    // Writer Identity (authenticated caller)
    pub writer_method: Option<String>,
    pub writer_subject: Option<String>,
    pub writer_issuer: Option<String>,

    // Process Identity
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub service_instance_id: Option<String>,
    pub process_pid: Option<i64>,
    pub process_owner: Option<String>,
    pub host_name: Option<String>,
    pub host_arch: Option<String>,

    // Network Identity (server-injected)
    pub client_address: Option<String>,
    pub client_port: Option<i64>,

    // Environment
    pub env: Option<std::collections::HashMap<String, String>>,

    // SDK Identity
    pub sdk_name: Option<String>,
    pub sdk_version: Option<String>,

    // Timestamps
    pub captured_at: Option<i64>,
}

/// Cached context metadata extracted from the first turn of a context.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ContextMetadata {
    pub client_tag: Option<String>,
    pub title: Option<String>,
    pub labels: Option<Vec<String>>,
    pub provenance: Option<Provenance>,
}

/// Result of a CQL search query.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub context_ids: Vec<u64>,
    pub total_count: usize,
    pub query: CqlQuery,
    pub elapsed_ms: u64,
}

pub struct Store {
    pub blob_store: BlobStore,
    pub turn_store: TurnStore,
    pub fs_roots: FsRootsIndex,
    /// Cache of context metadata, populated lazily from first turn.
    /// None value means we checked but found no metadata.
    pub context_metadata_cache: HashMap<u64, Option<ContextMetadata>>,
    /// Secondary indexes for CQL queries.
    secondary_indexes: SecondaryIndexes,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        let mut store = Self {
            blob_store: BlobStore::open(&dir.join("blobs"))?,
            turn_store: TurnStore::open(&dir.join("turns"))?,
            fs_roots: FsRootsIndex::open(&dir.join("fs"))?,
            context_metadata_cache: HashMap::new(),
            secondary_indexes: SecondaryIndexes::new(),
        };

        // Pre-populate metadata cache and build secondary indexes
        store.build_indexes();

        Ok(store)
    }

    /// Build secondary indexes from existing data.
    fn build_indexes(&mut self) {
        // Get all context heads
        let heads = self.turn_store.list_recent_contexts(u32::MAX);

        // Pre-populate metadata cache for all contexts
        for head in &heads {
            let _ = self.get_context_metadata(head.context_id);
        }

        // Build secondary indexes from the cache
        self.secondary_indexes
            .build_from_cache(&self.context_metadata_cache, &heads);
    }

    /// Get cached context metadata, loading from first turn if not cached.
    pub fn get_context_metadata(&mut self, context_id: u64) -> Option<ContextMetadata> {
        // Check cache first
        if let Some(cached) = self.context_metadata_cache.get(&context_id) {
            return cached.clone();
        }

        // Try to load from first turn (depth=0)
        let metadata = self.load_context_metadata(context_id);
        self.context_metadata_cache
            .insert(context_id, metadata.clone());
        metadata
    }

    /// Load context metadata from the first turn of a context.
    fn load_context_metadata(&mut self, context_id: u64) -> Option<ContextMetadata> {
        // Get the first turn (depth=0) for this context
        let first_turn = self.turn_store.get_first_turn(context_id).ok()?;
        let payload = self.blob_store.get(&first_turn.payload_hash).ok()?;
        extract_context_metadata(&payload)
    }

    /// Update the metadata cache when a new first turn is appended.
    /// Returns the extracted metadata if this is the first turn (depth=0).
    fn maybe_cache_metadata(
        &mut self,
        context_id: u64,
        depth: u32,
        payload: &[u8],
    ) -> Option<ContextMetadata> {
        if depth == 0 {
            let metadata = extract_context_metadata(payload);
            self.context_metadata_cache
                .insert(context_id, metadata.clone());
            metadata
        } else {
            None
        }
    }

    pub fn create_context(&mut self, base_turn_id: u64) -> Result<ContextHead> {
        self.turn_store.create_context(base_turn_id)
    }

    pub fn fork_context(&mut self, base_turn_id: u64) -> Result<ContextHead> {
        self.turn_store.fork_context(base_turn_id)
    }

    pub fn get_head(&self, context_id: u64) -> Result<ContextHead> {
        self.turn_store.get_head(context_id)
    }

    /// Append a turn to a context.
    ///
    /// Returns the turn record and, if this is the first turn (depth=0), the extracted metadata.
    #[allow(clippy::too_many_arguments)]
    pub fn append_turn(
        &mut self,
        context_id: u64,
        parent_turn_id: u64,
        declared_type_id: String,
        declared_type_version: u32,
        encoding: u32,
        compression: u32,
        uncompressed_len: u32,
        content_hash: [u8; 32],
        payload_bytes: &[u8],
    ) -> Result<(TurnRecord, Option<ContextMetadata>)> {
        let raw_bytes = match compression {
            0 => payload_bytes.to_vec(),
            1 => zstd::decode_all(payload_bytes)
                .map_err(|e| StoreError::InvalidInput(format!("zstd decode failed: {e}")))?,
            other => {
                return Err(StoreError::InvalidInput(format!(
                    "unsupported compression: {other}"
                )))
            }
        };

        if raw_bytes.len() as u32 != uncompressed_len {
            return Err(StoreError::InvalidInput(
                "uncompressed length mismatch".into(),
            ));
        }

        let mut hasher = Hasher::new();
        hasher.update(&raw_bytes);
        let hash = hasher.finalize();
        if hash.as_bytes() != &content_hash {
            return Err(StoreError::InvalidInput("content hash mismatch".into()));
        }

        self.blob_store.put_if_absent(content_hash, &raw_bytes)?;

        let record = self.turn_store.append_turn(
            context_id,
            parent_turn_id,
            content_hash,
            encoding,
            declared_type_id,
            declared_type_version,
            compression,
            uncompressed_len,
        )?;

        // Cache metadata if this is the first turn, and return it for event publishing
        let metadata = self.maybe_cache_metadata(context_id, record.depth, &raw_bytes);

        // Update secondary indexes if this is the first turn (depth=0)
        if record.depth == 0 {
            let head = self.turn_store.get_head(context_id)?;
            self.secondary_indexes.add_context(
                context_id,
                metadata.as_ref(),
                head.created_at_unix_ms,
                record.depth,
            );
        }

        Ok((record, metadata))
    }

    pub fn get_last(
        &mut self,
        context_id: u64,
        limit: u32,
        include_payload: bool,
    ) -> Result<Vec<TurnWithMeta>> {
        let turns = self.turn_store.get_last(context_id, limit)?;
        let mut out = Vec::with_capacity(turns.len());
        for record in turns {
            let meta = self.turn_store.get_turn_meta(record.turn_id)?;
            let payload = if include_payload {
                Some(self.blob_store.get(&record.payload_hash)?)
            } else {
                None
            };
            out.push(TurnWithMeta {
                record,
                meta,
                payload,
            });
        }
        Ok(out)
    }

    pub fn get_before(
        &mut self,
        context_id: u64,
        before_turn_id: u64,
        limit: u32,
        include_payload: bool,
    ) -> Result<Vec<TurnWithMeta>> {
        let turns = self
            .turn_store
            .get_before(context_id, before_turn_id, limit)?;
        let mut out = Vec::with_capacity(turns.len());
        for record in turns {
            let meta = self.turn_store.get_turn_meta(record.turn_id)?;
            let payload = if include_payload {
                Some(self.blob_store.get(&record.payload_hash)?)
            } else {
                None
            };
            out.push(TurnWithMeta {
                record,
                meta,
                payload,
            });
        }
        Ok(out)
    }

    pub fn get_blob(&mut self, hash: &[u8; 32]) -> Result<Vec<u8>> {
        self.blob_store.get(hash)
    }

    pub fn list_recent_contexts(&self, limit: u32) -> Vec<ContextHead> {
        self.turn_store.list_recent_contexts(limit)
    }

    /// Return direct child context IDs for a parent context.
    ///
    /// Child relationships are derived from first-turn provenance
    /// (`provenance.parent_context_id`) and maintained in secondary indexes.
    pub fn child_context_ids(&self, parent_context_id: u64) -> Vec<u64> {
        let mut ids: Vec<u64> = self
            .secondary_indexes
            .lookup_parent_exact(parent_context_id)
            .into_iter()
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids
    }

    /// Return descendant context IDs (children, grandchildren, ...) for a parent context.
    ///
    /// Results are deduplicated and sorted by context ID descending.
    pub fn descendant_context_ids(&self, parent_context_id: u64, limit: Option<u32>) -> Vec<u64> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<u64> = self.child_context_ids(parent_context_id).into();

        while let Some(context_id) = queue.pop_front() {
            if !visited.insert(context_id) {
                continue;
            }
            out.push(context_id);

            if let Some(max) = limit {
                if out.len() >= max as usize {
                    break;
                }
            }

            for child in self.child_context_ids(context_id) {
                if !visited.contains(&child) {
                    queue.push_back(child);
                }
            }
        }

        out.sort_unstable_by(|a, b| b.cmp(a));
        out
    }

    // =========================================================================
    // CQL Search Methods
    // =========================================================================

    /// Search contexts using a CQL query string.
    pub fn search_contexts(
        &self,
        query: &str,
        live_contexts: &HashSet<u64>,
        limit: Option<u32>,
    ) -> std::result::Result<SearchResult, CqlError> {
        let start = std::time::Instant::now();

        // Parse the query
        let parsed = cql::parse(query)?;

        // Execute the query
        let matching_ids = cql::execute(&parsed.ast, &self.secondary_indexes, live_contexts)?;

        // Sort by context_id descending (most recent first) and apply limit
        let mut sorted_ids: Vec<u64> = matching_ids.into_iter().collect();
        sorted_ids.sort_by(|a, b| b.cmp(a));

        let total_count = sorted_ids.len();
        if let Some(limit) = limit {
            sorted_ids.truncate(limit as usize);
        }

        let elapsed = start.elapsed();

        Ok(SearchResult {
            context_ids: sorted_ids,
            total_count,
            query: parsed,
            elapsed_ms: elapsed.as_millis() as u64,
        })
    }

    /// Search contexts using a pre-parsed CQL query.
    pub fn search_contexts_parsed(
        &self,
        query: &CqlQuery,
        live_contexts: &HashSet<u64>,
        limit: Option<u32>,
    ) -> std::result::Result<SearchResult, CqlError> {
        let start = std::time::Instant::now();

        // Execute the query
        let matching_ids = cql::execute(&query.ast, &self.secondary_indexes, live_contexts)?;

        // Sort by context_id descending (most recent first) and apply limit
        let mut sorted_ids: Vec<u64> = matching_ids.into_iter().collect();
        sorted_ids.sort_by(|a, b| b.cmp(a));

        let total_count = sorted_ids.len();
        if let Some(limit) = limit {
            sorted_ids.truncate(limit as usize);
        }

        let elapsed = start.elapsed();

        Ok(SearchResult {
            context_ids: sorted_ids,
            total_count,
            query: query.clone(),
            elapsed_ms: elapsed.as_millis() as u64,
        })
    }

    /// Get secondary index statistics.
    pub fn index_stats(&self) -> IndexStats {
        self.secondary_indexes.stats()
    }

    // =========================================================================
    // Filesystem Snapshot Methods
    // =========================================================================

    /// Attach a filesystem snapshot to a turn.
    /// The tree objects and file blobs must already exist in the blob store.
    pub fn attach_fs(&mut self, turn_id: u64, fs_root_hash: [u8; 32]) -> Result<()> {
        // Verify the turn exists
        let _ = self.turn_store.get_turn(turn_id)?;

        // Verify the root tree exists in blob store
        if !self.blob_store.contains(&fs_root_hash) {
            return Err(StoreError::NotFound("fs root tree blob".into()));
        }

        self.fs_roots.attach(turn_id, fs_root_hash)
    }

    /// Get the filesystem root hash for a turn (direct or inherited).
    pub fn get_fs_root(&self, turn_id: u64) -> Option<[u8; 32]> {
        self.fs_roots.get_inherited(turn_id, &self.turn_store)
    }

    /// Get the filesystem root hash directly attached to a turn (no inheritance).
    pub fn get_fs_root_direct(&self, turn_id: u64) -> Option<[u8; 32]> {
        self.fs_roots.get(turn_id)
    }

    /// List entries at a path in the filesystem snapshot for a turn.
    pub fn list_fs_entries(&mut self, turn_id: u64, path: &str) -> Result<Vec<TreeEntry>> {
        let fs_root = self
            .fs_roots
            .get_inherited(turn_id, &self.turn_store)
            .ok_or_else(|| StoreError::NotFound("no fs snapshot for turn".into()))?;

        let (tree_hash, is_dir) =
            crate::fs_store::resolve_path(&mut self.blob_store, &fs_root, path)?;

        if !is_dir {
            return Err(StoreError::InvalidInput(format!(
                "path is not a directory: {path}"
            )));
        }

        crate::fs_store::load_tree_entries(&mut self.blob_store, &tree_hash)
    }

    /// Get file content at a path in the filesystem snapshot for a turn.
    pub fn get_fs_file(&mut self, turn_id: u64, path: &str) -> Result<(Vec<u8>, TreeEntry)> {
        let fs_root = self
            .fs_roots
            .get_inherited(turn_id, &self.turn_store)
            .ok_or_else(|| StoreError::NotFound("no fs snapshot for turn".into()))?;

        crate::fs_store::get_file_at_path(&mut self.blob_store, &fs_root, path)
    }

    pub fn stats(&mut self) -> StoreStats {
        let blob_stats = self.blob_store.stats();
        let turn_stats = self.turn_store.stats();
        let fs_stats = self.fs_roots.stats();
        let fs_content_bytes = self.compute_fs_content_bytes();
        StoreStats {
            turns_total: turn_stats.turns_total,
            contexts_total: turn_stats.contexts_total,
            heads_total: turn_stats.heads_total,
            blobs_total: blob_stats.blobs_total,
            turns_log_bytes: turn_stats.turns_log_bytes,
            turns_index_bytes: turn_stats.turns_index_bytes,
            turns_meta_bytes: turn_stats.turns_meta_bytes,
            heads_table_bytes: turn_stats.heads_table_bytes,
            blobs_pack_bytes: blob_stats.pack_bytes,
            blobs_index_bytes: blob_stats.idx_bytes,
            fs_roots_total: fs_stats.entries_total,
            fs_roots_bytes: fs_stats.file_bytes,
            fs_content_bytes,
        }
    }

    /// Compute the total size of all blobs referenced by filesystem snapshots.
    /// This traverses all unique filesystem root trees and sums the raw blob sizes.
    fn compute_fs_content_bytes(&mut self) -> u64 {
        use std::collections::HashSet;

        let unique_roots = self.fs_roots.unique_roots();
        if unique_roots.is_empty() {
            return 0;
        }

        let mut visited: HashSet<[u8; 32]> = HashSet::new();
        let mut total_bytes: u64 = 0;

        for root_hash in unique_roots {
            total_bytes += self.compute_tree_size(&root_hash, &mut visited);
        }

        total_bytes
    }

    /// Recursively compute the size of all blobs in a tree.
    fn compute_tree_size(
        &mut self,
        tree_hash: &[u8; 32],
        visited: &mut std::collections::HashSet<[u8; 32]>,
    ) -> u64 {
        // Skip if already visited (deduplication)
        if !visited.insert(*tree_hash) {
            return 0;
        }

        // Add the tree blob's own size
        let tree_size = self.blob_store.raw_len(tree_hash).unwrap_or(0) as u64;

        // Try to load and traverse tree entries
        let entries = match crate::fs_store::load_tree_entries(&mut self.blob_store, tree_hash) {
            Ok(e) => e,
            Err(_) => return tree_size, // Can't parse tree, just return its own size
        };

        let mut total = tree_size;

        for entry in entries {
            if let Ok(hash) = entry.hash_array() {
                if entry.kind == 1 {
                    // Directory - recurse
                    total += self.compute_tree_size(&hash, visited);
                } else {
                    // File or symlink - add blob size if not visited
                    if visited.insert(hash) {
                        total += self.blob_store.raw_len(&hash).unwrap_or(0) as u64;
                    }
                }
            }
        }

        total
    }
}

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub turns_total: usize,
    pub contexts_total: usize,
    pub heads_total: usize,
    pub blobs_total: usize,
    pub turns_log_bytes: u64,
    pub turns_index_bytes: u64,
    pub turns_meta_bytes: u64,
    pub heads_table_bytes: u64,
    pub blobs_pack_bytes: u64,
    pub blobs_index_bytes: u64,
    pub fs_roots_total: usize,
    pub fs_roots_bytes: u64,
    pub fs_content_bytes: u64,
}

/// Extract context metadata from a msgpack-encoded ConversationItem payload.
///
/// The payload is expected to be a msgpack map with numeric keys.
/// We look for key 30 (context_metadata) which should contain:
/// - key 1: client_tag (string)
/// - key 2: title (string)
/// - key 3: labels (array of strings)
/// - key 10: provenance (nested map with provenance fields)
fn extract_context_metadata(payload: &[u8]) -> Option<ContextMetadata> {
    let mut cursor = std::io::Cursor::new(payload);
    let value = rmpv::decode::read_value(&mut cursor).ok()?;

    let map = match &value {
        Value::Map(m) => m,
        _ => return None,
    };

    // Find key 30 (context_metadata)
    let context_metadata_value =
        map.iter()
            .find_map(|(k, v)| if key_to_tag(k)? == 30 { Some(v) } else { None })?;

    let metadata_map = match context_metadata_value {
        Value::Map(m) => m,
        _ => return None,
    };

    let mut metadata = ContextMetadata::default();

    for (k, v) in metadata_map.iter() {
        let key = match key_to_tag(k) {
            Some(t) => t,
            None => continue,
        };

        match key {
            1 => {
                // client_tag
                if let Value::String(s) = v {
                    metadata.client_tag = s.as_str().map(|s| s.to_string());
                }
            }
            2 => {
                // title
                if let Value::String(s) = v {
                    metadata.title = s.as_str().map(|s| s.to_string());
                }
            }
            3 => {
                // labels
                if let Value::Array(arr) = v {
                    let labels: Vec<String> = arr
                        .iter()
                        .filter_map(|item| {
                            if let Value::String(s) = item {
                                s.as_str().map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !labels.is_empty() {
                        metadata.labels = Some(labels);
                    }
                }
            }
            10 => {
                // provenance
                if let Value::Map(prov_map) = v {
                    metadata.provenance = Some(extract_provenance(prov_map));
                }
            }
            _ => {}
        }
    }

    // Only return if we found at least one piece of metadata
    if metadata.client_tag.is_some()
        || metadata.title.is_some()
        || metadata.labels.is_some()
        || metadata.provenance.is_some()
    {
        Some(metadata)
    } else {
        None
    }
}

/// Extract provenance from a msgpack map.
fn extract_provenance(prov_map: &[(Value, Value)]) -> Provenance {
    let mut prov = Provenance::default();

    for (k, v) in prov_map.iter() {
        let key = match key_to_tag(k) {
            Some(t) => t,
            None => continue,
        };

        match key {
            // Context Lineage
            1 => prov.parent_context_id = extract_u64(v),
            2 => prov.spawn_reason = extract_string(v),
            3 => prov.root_context_id = extract_u64(v),

            // Request Identity
            10 => prov.trace_id = extract_string(v),
            11 => prov.span_id = extract_string(v),
            12 => prov.correlation_id = extract_string(v),

            // User Identity
            20 => prov.on_behalf_of = extract_string(v),
            21 => prov.on_behalf_of_source = extract_string(v),
            22 => prov.on_behalf_of_email = extract_string(v),

            // Writer Identity
            30 => prov.writer_method = extract_string(v),
            31 => prov.writer_subject = extract_string(v),
            32 => prov.writer_issuer = extract_string(v),

            // Process Identity
            40 => prov.service_name = extract_string(v),
            41 => prov.service_version = extract_string(v),
            42 => prov.service_instance_id = extract_string(v),
            43 => prov.process_pid = extract_i64(v),
            44 => prov.process_owner = extract_string(v),
            45 => prov.host_name = extract_string(v),
            46 => prov.host_arch = extract_string(v),

            // Network Identity (usually set server-side, but client may provide)
            50 => prov.client_address = extract_string(v),
            51 => prov.client_port = extract_i64(v),

            // Environment
            60 => prov.env = extract_string_map(v),

            // SDK Identity
            70 => prov.sdk_name = extract_string(v),
            71 => prov.sdk_version = extract_string(v),

            // Timestamps
            80 => prov.captured_at = extract_i64(v),

            _ => {}
        }
    }

    prov
}

/// Interpret a msgpack map key as a numeric tag.
/// Accepts both integer keys and string-encoded integers (e.g., "30").
/// Matches the projection layer's key_to_tag behavior (CLIENT_SPEC.md §3.1).
fn key_to_tag(key: &Value) -> Option<u64> {
    match key {
        Value::Integer(int) => int.as_u64().or_else(|| {
            int.as_i64()
                .and_then(|v| if v >= 0 { Some(v as u64) } else { None })
        }),
        Value::String(s) => s.as_str()?.parse::<u64>().ok(),
        _ => None,
    }
}

fn extract_string(v: &Value) -> Option<String> {
    if let Value::String(s) = v {
        s.as_str().map(|s| s.to_string())
    } else {
        None
    }
}

fn extract_u64(v: &Value) -> Option<u64> {
    if let Value::Integer(i) = v {
        i.as_u64()
    } else {
        None
    }
}

fn extract_i64(v: &Value) -> Option<i64> {
    if let Value::Integer(i) = v {
        i.as_i64()
    } else {
        None
    }
}

fn extract_string_map(v: &Value) -> Option<HashMap<String, String>> {
    if let Value::Map(m) = v {
        let map: HashMap<String, String> = m
            .iter()
            .filter_map(|(k, v)| {
                let key = extract_string(k)?;
                let val = extract_string(v)?;
                Some((key, val))
            })
            .collect();
        if map.is_empty() {
            None
        } else {
            Some(map)
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpv::Value;

    fn str_val(s: &str) -> Value {
        Value::String(s.into())
    }

    fn int_val(n: u64) -> Value {
        Value::Integer(rmpv::Integer::from(n))
    }

    #[test]
    fn key_to_tag_accepts_integer_keys() {
        assert_eq!(key_to_tag(&int_val(30)), Some(30));
        assert_eq!(key_to_tag(&int_val(0)), Some(0));
        assert_eq!(key_to_tag(&int_val(80)), Some(80));
    }

    #[test]
    fn key_to_tag_accepts_string_encoded_integers() {
        assert_eq!(key_to_tag(&str_val("30")), Some(30));
        assert_eq!(key_to_tag(&str_val("1")), Some(1));
        assert_eq!(key_to_tag(&str_val("80")), Some(80));
        assert_eq!(key_to_tag(&str_val("0")), Some(0));
    }

    #[test]
    fn key_to_tag_rejects_non_numeric_strings() {
        assert_eq!(key_to_tag(&str_val("hello")), None);
        assert_eq!(key_to_tag(&str_val("")), None);
        assert_eq!(key_to_tag(&str_val("-1")), None);
    }

    #[test]
    fn key_to_tag_rejects_other_types() {
        assert_eq!(key_to_tag(&Value::Boolean(true)), None);
        assert_eq!(key_to_tag(&Value::Nil), None);
    }

    /// Build a msgpack payload where the outer map uses string keys and
    /// the context_metadata inner maps also use string keys — matching
    /// what Go's msgpack encoder produces.
    fn encode_context_metadata_with_string_keys(
        client_tag: &str,
        service_name: &str,
        correlation_id: &str,
    ) -> Vec<u8> {
        let provenance = Value::Map(vec![
            (str_val("40"), str_val(service_name)),
            (str_val("12"), str_val(correlation_id)),
        ]);
        let context_metadata = Value::Map(vec![
            (str_val("1"), str_val(client_tag)),
            (str_val("10"), provenance),
        ]);
        let payload = Value::Map(vec![(str_val("30"), context_metadata)]);
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &payload).unwrap();
        buf
    }

    fn encode_context_metadata_with_integer_keys(client_tag: &str, service_name: &str) -> Vec<u8> {
        let provenance = Value::Map(vec![(int_val(40), str_val(service_name))]);
        let context_metadata = Value::Map(vec![
            (int_val(1), str_val(client_tag)),
            (int_val(10), provenance),
        ]);
        let payload = Value::Map(vec![(int_val(30), context_metadata)]);
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &payload).unwrap();
        buf
    }

    #[test]
    fn extract_context_metadata_with_string_keys() {
        let payload =
            encode_context_metadata_with_string_keys("kilroy/run-123", "kilroy", "run-123");
        let meta = extract_context_metadata(&payload).expect("should extract metadata");
        assert_eq!(meta.client_tag.as_deref(), Some("kilroy/run-123"));

        let prov = meta.provenance.expect("should have provenance");
        assert_eq!(prov.service_name.as_deref(), Some("kilroy"));
        assert_eq!(prov.correlation_id.as_deref(), Some("run-123"));
    }

    #[test]
    fn extract_context_metadata_with_integer_keys() {
        let payload = encode_context_metadata_with_integer_keys("test-tag", "my-service");
        let meta = extract_context_metadata(&payload).expect("should extract metadata");
        assert_eq!(meta.client_tag.as_deref(), Some("test-tag"));

        let prov = meta.provenance.expect("should have provenance");
        assert_eq!(prov.service_name.as_deref(), Some("my-service"));
    }
}
