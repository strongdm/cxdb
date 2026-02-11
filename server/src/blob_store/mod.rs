// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use crc32fast::Hasher;

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
        self.pack_file.flush()?;

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
        self.idx_file.flush()?;

        let entry = BlobIndexEntry {
            offset,
            raw_len,
            stored_len,
            codec,
        };
        self.index.insert(hash, entry.clone());
        Ok(entry)
    }

    pub fn get(&mut self, hash: &[u8; 32]) -> Result<Vec<u8>> {
        let entry = self
            .index
            .get(hash)
            .ok_or_else(|| StoreError::NotFound("blob".into()))?
            .clone();

        self.pack_file.seek(SeekFrom::Start(entry.offset))?;

        let magic = self.pack_file.read_u32::<LittleEndian>()?;
        if magic != BLOB_MAGIC {
            return Err(StoreError::Corrupt("invalid blob magic".into()));
        }
        let version = self.pack_file.read_u16::<LittleEndian>()?;
        if version != BLOB_VERSION {
            return Err(StoreError::Corrupt("unsupported blob version".into()));
        }
        let codec_raw = self.pack_file.read_u16::<LittleEndian>()?;
        let raw_len = self.pack_file.read_u32::<LittleEndian>()?;
        let stored_len = self.pack_file.read_u32::<LittleEndian>()?;
        let mut stored_hash = [0u8; 32];
        self.pack_file.read_exact(&mut stored_hash)?;

        if &stored_hash != hash {
            return Err(StoreError::Corrupt("blob hash mismatch".into()));
        }

        let mut stored_bytes = vec![0u8; stored_len as usize];
        self.pack_file.read_exact(&mut stored_bytes)?;
        let crc = self.pack_file.read_u32::<LittleEndian>()?;

        let mut header = Vec::with_capacity(4 + 2 + 2 + 4 + 4 + 32);
        header.write_u32::<LittleEndian>(magic)?;
        header.write_u16::<LittleEndian>(version)?;
        header.write_u16::<LittleEndian>(codec_raw)?;
        header.write_u32::<LittleEndian>(raw_len)?;
        header.write_u32::<LittleEndian>(stored_len)?;
        header.extend_from_slice(&stored_hash);

        let mut hasher = Hasher::new();
        hasher.update(&header);
        hasher.update(&stored_bytes);
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
            BlobCodec::None => stored_bytes,
            BlobCodec::Zstd => zstd::decode_all(&stored_bytes[..])
                .map_err(|e| StoreError::Corrupt(format!("zstd decode failed: {e}")))?,
        };

        if raw_bytes.len() as u32 != raw_len {
            return Err(StoreError::Corrupt("blob length mismatch".into()));
        }

        Ok(raw_bytes)
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

#[derive(Debug, Clone)]
pub struct BlobStoreStats {
    pub blobs_total: usize,
    pub pack_bytes: u64,
    pub idx_bytes: u64,
}

fn file_len(path: &PathBuf) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}
