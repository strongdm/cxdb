// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use crc32fast::Hasher;
use rayon::prelude::*;

use crate::error::{Result, StoreError};

const BLOB_MAGIC: u32 = 0x42534C42; // 'B''S''L''B'
const BLOB_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobCodec {
    None = 0,
    Zstd = 1,
}

#[derive(Debug, Clone)]
pub struct BlobIndexEntry {
    pub offset: u64,
    pub raw_len: u32,
    pub stored_len: u32,
    pub codec: BlobCodec,
}

pub struct BlobStore {
    pack_path: PathBuf,
    idx_path: PathBuf,
    pack_file: File,
    /// Separate read-only handle for pread-based concurrent reads.
    pack_read: File,
    idx_file: File,
    index: HashMap<[u8; 32], BlobIndexEntry>,
}

impl BlobStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let pack_path = dir.join("blobs.pack");
        let idx_path = dir.join("blobs.idx");

        let pack_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&pack_path)?;

        let pack_read = OpenOptions::new().read(true).open(&pack_path)?;

        let idx_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&idx_path)?;

        let mut store = Self {
            pack_path,
            idx_path,
            pack_file,
            pack_read,
            idx_file,
            index: HashMap::new(),
        };

        store.load_index()?;
        Ok(store)
    }

    fn load_index(&mut self) -> Result<()> {
        self.idx_file.seek(SeekFrom::Start(0))?;
        let mut buf = Vec::new();
        self.idx_file.read_to_end(&mut buf)?;

        // Each index entry is 52 bytes: hash(32) + offset(8) + raw_len(4) + stored_len(4) + codec(2) + reserved(2)
        const ENTRY_SIZE: usize = 32 + 8 + 4 + 4 + 2 + 2;

        let mut cursor = std::io::Cursor::new(&buf);
        let mut valid_len: u64 = 0;

        while (cursor.position() as usize) < buf.len() {
            let entry_start = cursor.position();

            // Check if we have enough bytes for a complete entry
            let remaining = buf.len() - entry_start as usize;
            if remaining < ENTRY_SIZE {
                // Partial entry - truncate and stop
                break;
            }

            let mut hash = [0u8; 32];
            if cursor.read_exact(&mut hash).is_err() {
                break;
            }

            // These reads should not fail given the size check above, but handle gracefully
            let offset = match cursor.read_u64::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => break,
            };
            let raw_len = match cursor.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => break,
            };
            let stored_len = match cursor.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => break,
            };
            let codec_raw = match cursor.read_u16::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => break,
            };
            let _reserved = match cursor.read_u16::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => break,
            };

            let codec = match codec_raw {
                0 => BlobCodec::None,
                1 => BlobCodec::Zstd,
                _ => return Err(StoreError::Corrupt("unknown blob codec".into())),
            };

            self.index.insert(
                hash,
                BlobIndexEntry {
                    offset,
                    raw_len,
                    stored_len,
                    codec,
                },
            );

            valid_len = cursor.position();
        }

        // Truncate any partial entry at the end
        if valid_len < buf.len() as u64 {
            self.idx_file.set_len(valid_len)?;
        }

        Ok(())
    }

    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.index.contains_key(hash)
    }

    pub fn put_if_absent(&mut self, hash: [u8; 32], raw_bytes: &[u8]) -> Result<BlobIndexEntry> {
        if let Some(entry) = self.index.get(&hash) {
            return Ok(entry.clone());
        }

        let mut stored_bytes = raw_bytes.to_vec();
        let mut codec = BlobCodec::None;
        if let Ok(compressed) = zstd::encode_all(raw_bytes, 1) {
            if compressed.len() < raw_bytes.len() {
                stored_bytes = compressed;
                codec = BlobCodec::Zstd;
            }
        }

        let raw_len = raw_bytes.len() as u32;
        let stored_len = stored_bytes.len() as u32;

        let offset = self.pack_file.seek(SeekFrom::End(0))?;

        let mut header = Vec::with_capacity(4 + 2 + 2 + 4 + 4 + 32);
        header.write_u32::<LittleEndian>(BLOB_MAGIC)?;
        header.write_u16::<LittleEndian>(BLOB_VERSION)?;
        header.write_u16::<LittleEndian>(codec as u16)?;
        header.write_u32::<LittleEndian>(raw_len)?;
        header.write_u32::<LittleEndian>(stored_len)?;
        header.extend_from_slice(&hash);

        let mut hasher = Hasher::new();
        hasher.update(&header);
        hasher.update(&stored_bytes);
        let crc = hasher.finalize();

        self.pack_file.write_all(&header)?;
        self.pack_file.write_all(&stored_bytes)?;
        self.pack_file.write_u32::<LittleEndian>(crc)?;
        self.pack_file.sync_all()?;

        // append to index
        let mut idx_entry = Vec::with_capacity(32 + 8 + 4 + 4 + 2 + 2);
        idx_entry.extend_from_slice(&hash);
        idx_entry.write_u64::<LittleEndian>(offset)?;
        idx_entry.write_u32::<LittleEndian>(raw_len)?;
        idx_entry.write_u32::<LittleEndian>(stored_len)?;
        idx_entry.write_u16::<LittleEndian>(codec as u16)?;
        idx_entry.write_u16::<LittleEndian>(0)?;
        self.idx_file.seek(SeekFrom::End(0))?;
        self.idx_file.write_all(&idx_entry)?;
        self.idx_file.sync_all()?;

        let entry = BlobIndexEntry {
            offset,
            raw_len,
            stored_len,
            codec,
        };
        self.index.insert(hash, entry.clone());
        Ok(entry)
    }

    /// Read a blob by hash. Uses pread (read_at) so this does not mutate
    /// the file offset and can safely be called from &self.
    pub fn get(&self, hash: &[u8; 32]) -> Result<Vec<u8>> {
        let entry = self
            .index
            .get(hash)
            .ok_or_else(|| StoreError::NotFound("blob".into()))?
            .clone();

        // Header: magic(4) + version(2) + codec(2) + raw_len(4) + stored_len(4) + hash(32) = 48 bytes
        const HEADER_SIZE: usize = 4 + 2 + 2 + 4 + 4 + 32;

        // Read header first, then validate it against the in-memory index before
        // allocating and slicing payload buffers.
        let mut header = [0u8; HEADER_SIZE];
        self.read_at_exact(entry.offset, &mut header)?;

        let mut cursor = std::io::Cursor::new(&header);
        let magic = cursor.read_u32::<LittleEndian>()?;
        if magic != BLOB_MAGIC {
            return Err(StoreError::Corrupt("invalid blob magic".into()));
        }
        let version = cursor.read_u16::<LittleEndian>()?;
        if version != BLOB_VERSION {
            return Err(StoreError::Corrupt("unsupported blob version".into()));
        }
        let codec_raw = cursor.read_u16::<LittleEndian>()?;
        let raw_len = cursor.read_u32::<LittleEndian>()?;
        let stored_len = cursor.read_u32::<LittleEndian>()?;
        let mut stored_hash = [0u8; 32];
        cursor.read_exact(&mut stored_hash)?;

        if &stored_hash != hash {
            return Err(StoreError::Corrupt("blob hash mismatch".into()));
        }

        if stored_len != entry.stored_len || raw_len != entry.raw_len {
            return Err(StoreError::Corrupt(
                "blob index/header length mismatch".into(),
            ));
        }

        let body_offset = entry
            .offset
            .checked_add(HEADER_SIZE as u64)
            .ok_or_else(|| StoreError::Corrupt("blob offset overflow".into()))?;
        let body_len = (stored_len as usize)
            .checked_add(4)
            .ok_or_else(|| StoreError::Corrupt("blob length overflow".into()))?;
        let mut body = vec![0u8; body_len];
        self.read_at_exact(body_offset, &mut body)?;

        let stored_bytes = &body[..stored_len as usize];
        let crc_offset = stored_len as usize;
        let crc = {
            let mut c = std::io::Cursor::new(&body[crc_offset..crc_offset + 4]);
            c.read_u32::<LittleEndian>()?
        };

        // Verify CRC over header + stored bytes
        let mut hasher = Hasher::new();
        hasher.update(&header);
        hasher.update(stored_bytes);
        let actual_crc = hasher.finalize();
        if crc != actual_crc {
            return Err(StoreError::Corrupt("blob crc mismatch".into()));
        }

        let codec = match codec_raw {
            0 => BlobCodec::None,
            1 => BlobCodec::Zstd,
            _ => return Err(StoreError::Corrupt("unknown blob codec".into())),
        };

        let raw_bytes = match codec {
            BlobCodec::None => stored_bytes.to_vec(),
            BlobCodec::Zstd => zstd::decode_all(stored_bytes)
                .map_err(|e| StoreError::Corrupt(format!("zstd decode failed: {e}")))?,
        };

        if raw_bytes.len() as u32 != raw_len {
            return Err(StoreError::Corrupt("blob length mismatch".into()));
        }
        if blake3::hash(&raw_bytes).as_bytes() != hash {
            return Err(StoreError::Corrupt("blob content hash mismatch".into()));
        }

        Ok(raw_bytes)
    }

    /// Read several blobs with bounded, coalesced pread operations.
    ///
    /// Records are decoded independently after the range read. The returned
    /// vector has exactly the same order and duplicates as `hashes`.
    pub fn get_many(&self, hashes: &[[u8; 32]]) -> Result<Vec<Vec<u8>>> {
        const HEADER_SIZE: u64 = 48;
        const CRC_SIZE: u64 = 4;
        const MAX_GAP: u64 = 64 * 1024;
        const MAX_RANGE: u64 = 16 * 1024 * 1024;

        if hashes.is_empty() {
            return Ok(Vec::new());
        }
        let mut unique = Vec::with_capacity(hashes.len());
        let mut seen = HashSet::with_capacity(hashes.len());
        for hash in hashes {
            if seen.insert(*hash) {
                let entry = self
                    .index
                    .get(hash)
                    .ok_or_else(|| StoreError::NotFound("blob".into()))?
                    .clone();
                let end = entry
                    .offset
                    .checked_add(HEADER_SIZE)
                    .and_then(|v| v.checked_add(u64::from(entry.stored_len)))
                    .and_then(|v| v.checked_add(CRC_SIZE))
                    .ok_or_else(|| StoreError::Corrupt("blob record offset overflow".into()))?;
                unique.push((*hash, entry, end));
            }
        }
        unique.sort_unstable_by_key(|(_, entry, _)| entry.offset);

        let pack_len = self.pack_read.metadata()?.len();
        let mut decoded = HashMap::with_capacity(unique.len());
        let mut first = 0;
        while first < unique.len() {
            let range_start = unique[first].1.offset;
            let mut range_end = unique[first].2;
            let mut last = first + 1;
            while last < unique.len() {
                let next = &unique[last];
                if next.1.offset < range_end {
                    return Err(StoreError::Corrupt("overlapping blob index entries".into()));
                }
                let gap = next.1.offset - range_end;
                let span = next
                    .2
                    .checked_sub(range_start)
                    .ok_or_else(|| StoreError::Corrupt("invalid blob index range".into()))?;
                if gap > MAX_GAP || span > MAX_RANGE {
                    break;
                }
                range_end = next.2;
                last += 1;
            }
            if range_end > pack_len {
                return Err(StoreError::Corrupt(
                    "blob index points past pack end".into(),
                ));
            }
            let range_len = usize::try_from(range_end - range_start)
                .map_err(|_| StoreError::Corrupt("blob read range exceeds address space".into()))?;
            let mut range = vec![0u8; range_len];
            self.read_at_exact(range_start, &mut range)?;
            let group: Result<Vec<([u8; 32], Vec<u8>)>> = unique[first..last]
                .par_iter()
                .map(|(hash, entry, end)| {
                    let start = usize::try_from(entry.offset - range_start).map_err(|_| {
                        StoreError::Corrupt("blob offset exceeds address space".into())
                    })?;
                    let end = usize::try_from(*end - range_start).map_err(|_| {
                        StoreError::Corrupt("blob record exceeds address space".into())
                    })?;
                    let record = range.get(start..end).ok_or_else(|| {
                        StoreError::Corrupt("blob record outside read range".into())
                    })?;
                    Ok((*hash, decode_blob_record_slice(record, hash, entry)?))
                })
                .collect();
            for (hash, payload) in group? {
                decoded.insert(hash, payload);
            }
            first = last;
        }

        hashes
            .iter()
            .map(|hash| {
                decoded
                    .get(hash)
                    .cloned()
                    .ok_or_else(|| StoreError::NotFound("blob".into()))
            })
            .collect()
    }

    /// Read exactly buf.len() bytes from the read handle at the given offset using pread.
    fn read_at_exact(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let mut total_read = 0usize;
        while total_read < buf.len() {
            let n = self
                .pack_read
                .read_at(&mut buf[total_read..], offset + total_read as u64)
                .map_err(StoreError::Io)?;
            if n == 0 {
                return Err(StoreError::Corrupt("unexpected EOF reading blob".into()));
            }
            total_read += n;
        }
        Ok(())
    }

    pub fn stats(&self) -> BlobStoreStats {
        BlobStoreStats {
            blobs_total: self.index.len(),
            pack_bytes: file_len(&self.pack_path),
            idx_bytes: file_len(&self.idx_path),
        }
    }

    /// Get the raw (uncompressed) length of a blob without loading its content.
    pub fn raw_len(&self, hash: &[u8; 32]) -> Option<u32> {
        self.index.get(hash).map(|e| e.raw_len)
    }

    /// Get the stored (compressed) length of a blob without loading its content.
    pub fn stored_len(&self, hash: &[u8; 32]) -> Option<u32> {
        self.index.get(hash).map(|e| e.stored_len)
    }
}

fn decode_blob_record_slice(
    record: &[u8],
    expected_hash: &[u8; 32],
    expected_entry: &BlobIndexEntry,
) -> Result<Vec<u8>> {
    const HEADER_SIZE: usize = 48;
    let expected_len = HEADER_SIZE
        .checked_add(expected_entry.stored_len as usize)
        .and_then(|v| v.checked_add(4))
        .ok_or_else(|| StoreError::Corrupt("blob record length overflow".into()))?;
    if record.len() != expected_len {
        return Err(StoreError::Corrupt("blob record length mismatch".into()));
    }
    let mut header = Cursor::new(&record[..HEADER_SIZE]);
    let magic = header.read_u32::<LittleEndian>()?;
    let version = header.read_u16::<LittleEndian>()?;
    let codec_raw = header.read_u16::<LittleEndian>()?;
    let raw_len = header.read_u32::<LittleEndian>()?;
    let stored_len = header.read_u32::<LittleEndian>()?;
    let mut stored_hash = [0u8; 32];
    header.read_exact(&mut stored_hash)?;
    if magic != BLOB_MAGIC || version != BLOB_VERSION {
        return Err(StoreError::Corrupt("invalid blob header".into()));
    }
    if &stored_hash != expected_hash
        || raw_len != expected_entry.raw_len
        || stored_len != expected_entry.stored_len
        || codec_raw != expected_entry.codec as u16
    {
        return Err(StoreError::Corrupt("blob index/header mismatch".into()));
    }
    let stored_end = HEADER_SIZE + stored_len as usize;
    let stored = &record[HEADER_SIZE..stored_end];
    let crc = Cursor::new(&record[stored_end..]).read_u32::<LittleEndian>()?;
    let mut hasher = Hasher::new();
    hasher.update(&record[..stored_end]);
    if crc != hasher.finalize() {
        return Err(StoreError::Corrupt("blob crc mismatch".into()));
    }
    let raw = match expected_entry.codec {
        BlobCodec::None => stored.to_vec(),
        BlobCodec::Zstd => zstd::decode_all(stored)
            .map_err(|e| StoreError::Corrupt(format!("zstd decode failed: {e}")))?,
    };
    if raw.len() != raw_len as usize {
        return Err(StoreError::Corrupt("blob length mismatch".into()));
    }
    if blake3::hash(&raw).as_bytes() != expected_hash {
        return Err(StoreError::Corrupt("blob content hash mismatch".into()));
    }
    Ok(raw)
}

#[derive(Debug, Clone)]
pub struct BlobStoreStats {
    pub blobs_total: usize,
    pub pack_bytes: u64,
    pub idx_bytes: u64,
}

fn file_len(path: &PathBuf) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::BlobStore;
    use tempfile::tempdir;

    #[test]
    fn get_many_preserves_order_and_duplicates() {
        let dir = tempdir().expect("tempdir");
        let mut store = BlobStore::open(dir.path()).expect("open");
        let first = b"first payload";
        let second = b"second payload";
        let first_hash = *blake3::hash(first).as_bytes();
        let second_hash = *blake3::hash(second).as_bytes();
        store.put_if_absent(first_hash, first).expect("first");
        store.put_if_absent(second_hash, second).expect("second");
        let values = store
            .get_many(&[second_hash, first_hash, second_hash])
            .expect("batch read");
        assert_eq!(
            values,
            vec![second.to_vec(), first.to_vec(), second.to_vec()]
        );
    }
}
