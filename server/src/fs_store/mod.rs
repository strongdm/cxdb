// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

//! Filesystem snapshot storage for CXDB.
//!
//! This module provides a sparse index mapping `turn_id → fs_root_hash`, allowing
//! filesystem snapshots to be associated with conversation turns. Tree objects
//! (directory listings) are stored in the main blob store as msgpack-encoded data.
//!
//! # Storage Format
//!
//! The roots index (`fs/roots.idx`) is an append-only file with fixed-size records:
//! - turn_id: u64 (8 bytes)
//! - fs_root_hash: [u8; 32] (32 bytes)
//! - crc32: u32 (4 bytes)
//! - Total: 44 bytes per record
//!
//! Last-write-wins semantics per turn_id (like heads.tbl).
//!
//! # Tree Object Format
//!
//! Tree objects are msgpack arrays of TreeEntry, stored in the blob store:
//! ```text
//! TreeEntry {
//!     name: String,      // msgpack tag 1
//!     kind: u8,          // msgpack tag 2 (0=file, 1=dir, 2=symlink)
//!     mode: u32,         // msgpack tag 3 (POSIX permissions)
//!     size: u64,         // msgpack tag 4 (file size, 0 for dirs)
//!     hash: [u8; 32],    // msgpack tag 5 (content hash)
//! }
//! ```

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use crc32fast::Hasher;
use rmpv::Value;

use crate::blob_store::BlobStore;
use crate::error::{Result, StoreError};
use crate::turn_store::TurnStore;

/// Entry kinds for filesystem tree entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryKind {
    File = 0,
    Directory = 1,
    Symlink = 2,
}

impl From<u8> for EntryKind {
    fn from(v: u8) -> Self {
        match v {
            0 => EntryKind::File,
            1 => EntryKind::Directory,
            2 => EntryKind::Symlink,
            _ => EntryKind::File, // default to file for unknown
        }
    }
}

/// A single entry in a directory tree.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    /// Filename (no path separators).
    pub name: String,

    /// Entry kind (file, directory, symlink).
    pub kind: u8,

    /// POSIX permission bits (e.g., 0o755).
    pub mode: u32,

    /// Size in bytes (files only, 0 for directories).
    pub size: u64,

    /// BLAKE3-256 hash of content (file), subtree (dir), or target (symlink).
    pub hash: Vec<u8>,
}

impl TreeEntry {
    /// Get the hash as a fixed-size array.
    pub fn hash_array(&self) -> Result<[u8; 32]> {
        if self.hash.len() != 32 {
            return Err(StoreError::Corrupt(format!(
                "invalid hash length: {}",
                self.hash.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&self.hash);
        Ok(arr)
    }

    /// Get the entry kind enum.
    pub fn kind_enum(&self) -> EntryKind {
        EntryKind::from(self.kind)
    }
}

/// Sparse index mapping turn_id → fs_root_hash.
pub struct FsRootsIndex {
    path: PathBuf,
    file: File,
    roots: HashMap<u64, [u8; 32]>,
}

impl FsRootsIndex {
    /// Open or create the filesystem roots index.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("roots.idx");

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;

        let mut index = Self {
            path,
            file,
            roots: HashMap::new(),
        };

        index.load()?;
        Ok(index)
    }

    /// Load existing entries from disk.
    fn load(&mut self) -> Result<()> {
        self.roots.clear();
        self.file.seek(SeekFrom::Start(0))?;

        loop {
            let start = self.file.stream_position()?;

            // Read turn_id
            let turn_id = match self.file.read_u64::<LittleEndian>() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(StoreError::Io(e)),
            };

            // Read fs_root_hash
            let mut fs_root_hash = [0u8; 32];
            if self.file.read_exact(&mut fs_root_hash).is_err() {
                self.file.set_len(start)?;
                break;
            }

            // Read and verify CRC
            let crc = match self.file.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.file.set_len(start)?;
                    break;
                }
            };

            let actual_crc = Self::compute_crc(turn_id, &fs_root_hash);
            if crc != actual_crc {
                self.file.set_len(start)?;
                break;
            }

            self.roots.insert(turn_id, fs_root_hash);
        }

        Ok(())
    }

    /// Compute CRC32 for a record.
    fn compute_crc(turn_id: u64, fs_root_hash: &[u8; 32]) -> u32 {
        let mut buf = Vec::with_capacity(40);
        buf.write_u64::<LittleEndian>(turn_id).unwrap();
        buf.extend_from_slice(fs_root_hash);
        let mut hasher = Hasher::new();
        hasher.update(&buf);
        hasher.finalize()
    }

    /// Attach a filesystem snapshot to a turn.
    pub fn attach(&mut self, turn_id: u64, fs_root_hash: [u8; 32]) -> Result<()> {
        // Write record to file
        let mut buf = Vec::with_capacity(44);
        buf.write_u64::<LittleEndian>(turn_id)?;
        buf.extend_from_slice(&fs_root_hash);
        let crc = Self::compute_crc(turn_id, &fs_root_hash);
        buf.write_u32::<LittleEndian>(crc)?;

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&buf)?;
        self.file.flush()?;

        // Update in-memory index
        self.roots.insert(turn_id, fs_root_hash);

        Ok(())
    }

    /// Get the fs_root_hash directly attached to a turn.
    pub fn get(&self, turn_id: u64) -> Option<[u8; 32]> {
        self.roots.get(&turn_id).copied()
    }

    /// Get the fs_root_hash for a turn, walking parent chain if not directly attached.
    pub fn get_inherited(&self, turn_id: u64, turn_store: &TurnStore) -> Option<[u8; 32]> {
        // First check direct attachment
        if let Some(hash) = self.roots.get(&turn_id) {
            return Some(*hash);
        }

        // Walk parent chain
        let mut current = turn_id;
        while current != 0 {
            if let Ok(turn) = turn_store.get_turn(current) {
                if let Some(hash) = self.roots.get(&turn.turn_id) {
                    return Some(*hash);
                }
                current = turn.parent_turn_id;
            } else {
                break;
            }
        }

        None
    }

    /// Check if a turn has a filesystem snapshot (direct or inherited).
    pub fn has_snapshot(&self, turn_id: u64, turn_store: &TurnStore) -> bool {
        self.get_inherited(turn_id, turn_store).is_some()
    }

    /// Get statistics about the index.
    pub fn stats(&self) -> FsRootsStats {
        FsRootsStats {
            entries_total: self.roots.len(),
            file_bytes: std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0),
            content_bytes: 0, // Computed by Store::stats() which has blob_store access
        }
    }

    /// Get all unique root hashes for computing content size.
    pub fn unique_roots(&self) -> Vec<[u8; 32]> {
        let mut seen = std::collections::HashSet::new();
        let mut roots = Vec::new();
        for hash in self.roots.values() {
            if seen.insert(*hash) {
                roots.push(*hash);
            }
        }
        roots
    }
}

/// Statistics about the filesystem roots index.
#[derive(Debug, Clone)]
pub struct FsRootsStats {
    pub entries_total: usize,
    pub file_bytes: u64,
    /// Total size of all blobs referenced by filesystem snapshots (computed externally).
    pub content_bytes: u64,
}

/// Load and deserialize tree entries from the blob store.
pub fn load_tree_entries(
    blob_store: &mut BlobStore,
    tree_hash: &[u8; 32],
) -> Result<Vec<TreeEntry>> {
    let bytes = blob_store.get(tree_hash)?;
    parse_tree_entries(&bytes)
}

/// Parse tree entries from msgpack bytes.
/// The format is an array of maps with numeric keys (1=name, 2=kind, 3=mode, 4=size, 5=hash).
fn parse_tree_entries(bytes: &[u8]) -> Result<Vec<TreeEntry>> {
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value(&mut cursor)
        .map_err(|e| StoreError::Corrupt(format!("invalid tree msgpack: {e}")))?;

    let array = match &value {
        Value::Array(arr) => arr,
        _ => return Err(StoreError::Corrupt("tree is not an array".into())),
    };

    let mut entries = Vec::with_capacity(array.len());
    for item in array {
        let entry = parse_tree_entry(item)?;
        entries.push(entry);
    }

    Ok(entries)
}

/// Parse a single TreeEntry from a msgpack Value.
/// Supports both integer keys (1, 2, 3...) and string keys ("1", "2", "3"...)
/// since Go's msgpack encoder uses string keys for struct tags like `msgpack:"1"`.
fn parse_tree_entry(value: &Value) -> Result<TreeEntry> {
    let map = match value {
        Value::Map(m) => m,
        _ => return Err(StoreError::Corrupt("tree entry is not a map".into())),
    };

    let mut name = String::new();
    let mut kind: u8 = 0;
    let mut mode: u32 = 0;
    let mut size: u64 = 0;
    let mut hash: Vec<u8> = Vec::new();

    for (k, v) in map {
        // Support both integer keys and string keys (Go uses string keys like "1", "2")
        let key: u64 = match k {
            Value::Integer(i) => i.as_u64().unwrap_or(0),
            Value::String(s) => s.as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
            _ => continue,
        };

        match key {
            1 => {
                // name
                if let Value::String(s) = v {
                    name = s.as_str().unwrap_or("").to_string();
                }
            }
            2 => {
                // kind
                if let Value::Integer(i) = v {
                    kind = i.as_u64().unwrap_or(0) as u8;
                }
            }
            3 => {
                // mode
                if let Value::Integer(i) = v {
                    mode = i.as_u64().unwrap_or(0) as u32;
                }
            }
            4 => {
                // size
                if let Value::Integer(i) = v {
                    size = i.as_u64().unwrap_or(0);
                }
            }
            5 => {
                // hash
                if let Value::Binary(b) = v {
                    hash = b.clone();
                }
            }
            _ => {}
        }
    }

    Ok(TreeEntry {
        name,
        kind,
        mode,
        size,
        hash,
    })
}

/// Resolve a path to its tree hash (for directories) or blob hash (for files).
/// Returns (hash, is_directory).
pub fn resolve_path(
    blob_store: &mut BlobStore,
    root_hash: &[u8; 32],
    path: &str,
) -> Result<([u8; 32], bool)> {
    if path.is_empty() || path == "/" {
        return Ok((*root_hash, true));
    }

    let parts: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();

    if parts.is_empty() {
        return Ok((*root_hash, true));
    }

    let mut current_hash = *root_hash;

    for (i, part) in parts.iter().enumerate() {
        let entries = load_tree_entries(blob_store, &current_hash)?;

        let entry = entries
            .iter()
            .find(|e| e.name == *part)
            .ok_or_else(|| StoreError::NotFound(format!("path component not found: {part}")))?;

        let entry_hash = entry.hash_array()?;
        let is_last = i == parts.len() - 1;

        if is_last {
            return Ok((entry_hash, entry.kind_enum() == EntryKind::Directory));
        }

        // Must be a directory to continue
        if entry.kind_enum() != EntryKind::Directory {
            return Err(StoreError::InvalidInput(format!("not a directory: {part}")));
        }

        current_hash = entry_hash;
    }

    unreachable!()
}

/// Get a file's content by path from a filesystem snapshot.
pub fn get_file_at_path(
    blob_store: &mut BlobStore,
    root_hash: &[u8; 32],
    path: &str,
) -> Result<(Vec<u8>, TreeEntry)> {
    let parts: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();

    if parts.is_empty() {
        return Err(StoreError::InvalidInput("empty path".into()));
    }

    let mut current_hash = *root_hash;

    for (i, part) in parts.iter().enumerate() {
        let entries = load_tree_entries(blob_store, &current_hash)?;

        let entry = entries
            .iter()
            .find(|e| e.name == *part)
            .ok_or_else(|| StoreError::NotFound(format!("path component not found: {part}")))?;

        let entry_hash = entry.hash_array()?;
        let is_last = i == parts.len() - 1;

        if is_last {
            // Return file content
            match entry.kind_enum() {
                EntryKind::File => {
                    let content = blob_store.get(&entry_hash)?;
                    return Ok((content, entry.clone()));
                }
                EntryKind::Symlink => {
                    // For symlinks, return the target path as content
                    let content = blob_store.get(&entry_hash)?;
                    return Ok((content, entry.clone()));
                }
                EntryKind::Directory => {
                    return Err(StoreError::InvalidInput(format!(
                        "path is a directory: {path}"
                    )));
                }
            }
        }

        // Must be a directory to continue
        if entry.kind_enum() != EntryKind::Directory {
            return Err(StoreError::InvalidInput(format!("not a directory: {part}")));
        }

        current_hash = entry_hash;
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_fs_roots_index() {
        let tmpdir = TempDir::new().unwrap();
        let mut index = FsRootsIndex::open(tmpdir.path()).unwrap();

        // Initially empty
        assert!(index.get(1).is_none());

        // Attach
        let hash = [0xabu8; 32];
        index.attach(1, hash).unwrap();

        // Should be retrievable
        assert_eq!(index.get(1), Some(hash));

        // Reopen and verify persistence
        drop(index);
        let index2 = FsRootsIndex::open(tmpdir.path()).unwrap();
        assert_eq!(index2.get(1), Some(hash));
    }

    #[test]
    fn test_fs_roots_overwrite() {
        let tmpdir = TempDir::new().unwrap();
        let mut index = FsRootsIndex::open(tmpdir.path()).unwrap();

        let hash1 = [0x11u8; 32];
        let hash2 = [0x22u8; 32];

        index.attach(1, hash1).unwrap();
        index.attach(1, hash2).unwrap();

        // Last write wins
        assert_eq!(index.get(1), Some(hash2));
    }
}
