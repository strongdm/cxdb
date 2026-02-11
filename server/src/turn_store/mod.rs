// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use crc32fast::Hasher;

use crate::error::{Result, StoreError};

#[derive(Debug, Clone)]
pub struct TurnRecord {
    pub turn_id: u64,
    pub parent_turn_id: u64,
    pub depth: u32,
    pub codec: u32,
    pub type_tag: u64,
    pub payload_hash: [u8; 32],
    pub flags: u32,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
pub struct TurnMeta {
    pub declared_type_id: String,
    pub declared_type_version: u32,
    pub encoding: u32,
    pub compression: u32,
    pub uncompressed_len: u32,
}

#[derive(Debug, Clone)]
pub struct ContextHead {
    pub context_id: u64,
    pub head_turn_id: u64,
    pub head_depth: u32,
    pub created_at_unix_ms: u64,
    pub flags: u32,
}

pub struct TurnStore {
    turns_log_path: std::path::PathBuf,
    turns_idx_path: std::path::PathBuf,
    turns_meta_path: std::path::PathBuf,
    heads_tbl_path: std::path::PathBuf,

    turns_log: File,
    turns_idx: File,
    turns_meta: File,
    heads_tbl: File,

    turns: HashMap<u64, TurnRecord>,
    turn_index: HashMap<u64, u64>,
    turn_meta: HashMap<u64, TurnMeta>,
    heads: HashMap<u64, ContextHead>,

    next_turn_id: u64,
    next_context_id: u64,
}

impl TurnStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let turns_log_path = dir.join("turns.log");
        let turns_idx_path = dir.join("turns.idx");
        let turns_meta_path = dir.join("turns.meta");
        let heads_tbl_path = dir.join("heads.tbl");

        let turns_log = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&turns_log_path)?;
        let turns_idx = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&turns_idx_path)?;
        let turns_meta = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&turns_meta_path)?;
        let heads_tbl = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&heads_tbl_path)?;

        let mut store = Self {
            turns_log_path,
            turns_idx_path,
            turns_meta_path,
            heads_tbl_path,
            turns_log,
            turns_idx,
            turns_meta,
            heads_tbl,
            turns: HashMap::new(),
            turn_index: HashMap::new(),
            turn_meta: HashMap::new(),
            heads: HashMap::new(),
            next_turn_id: 1,
            next_context_id: 1,
        };

        store.load_turns()?;
        store.load_meta()?;
        store.load_heads()?;
        store.rebuild_index()?;
        store.update_counters();

        Ok(store)
    }

    pub fn stats(&self) -> TurnStoreStats {
        TurnStoreStats {
            turns_total: self.turns.len(),
            contexts_total: self.heads.len(),
            heads_total: self.heads.len(),
            turns_log_bytes: file_len(&self.turns_log_path),
            turns_index_bytes: file_len(&self.turns_idx_path),
            turns_meta_bytes: file_len(&self.turns_meta_path),
            heads_table_bytes: file_len(&self.heads_tbl_path),
        }
    }

    fn load_turns(&mut self) -> Result<()> {
        self.turns.clear();
        self.turn_index.clear();

        self.turns_log.seek(SeekFrom::Start(0))?;
        let mut offset = 0u64;
        loop {
            let start = self.turns_log.stream_position()?;
            let record = match read_turn_record(&mut self.turns_log) {
                Ok(rec) => rec,
                Err(StoreError::Corrupt(_)) => {
                    // truncate partial/corrupt tail
                    self.turns_log.set_len(start)?;
                    break;
                }
                Err(StoreError::Io(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Truncate partial record to allow future appends to work correctly
                    self.turns_log.set_len(start)?;
                    break;
                }
                Err(e) => return Err(e),
            };

            self.turns.insert(record.turn_id, record.clone());
            self.turn_index.insert(record.turn_id, offset);
            offset = self.turns_log.stream_position()?;
        }
        Ok(())
    }

    fn load_meta(&mut self) -> Result<()> {
        self.turn_meta.clear();
        self.turns_meta.seek(SeekFrom::Start(0))?;

        loop {
            let start = self.turns_meta.stream_position()?;
            let turn_id = match self.turns_meta.read_u64::<LittleEndian>() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(StoreError::Io(e)),
            };
            let len = match self.turns_meta.read_u32::<LittleEndian>() {
                Ok(v) => v as usize,
                Err(_) => {
                    self.turns_meta.set_len(start)?;
                    break;
                }
            };
            let mut buf = vec![0u8; len];
            if self.turns_meta.read_exact(&mut buf).is_err() {
                self.turns_meta.set_len(start)?;
                break;
            }
            let declared_type_id = String::from_utf8(buf)
                .map_err(|_| StoreError::Corrupt("invalid type id utf8".into()))?;
            let declared_type_version = match self.turns_meta.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.turns_meta.set_len(start)?;
                    break;
                }
            };
            let encoding = match self.turns_meta.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.turns_meta.set_len(start)?;
                    break;
                }
            };
            let compression = match self.turns_meta.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.turns_meta.set_len(start)?;
                    break;
                }
            };
            let uncompressed_len = match self.turns_meta.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.turns_meta.set_len(start)?;
                    break;
                }
            };

            self.turn_meta.insert(
                turn_id,
                TurnMeta {
                    declared_type_id,
                    declared_type_version,
                    encoding,
                    compression,
                    uncompressed_len,
                },
            );
        }

        Ok(())
    }

    fn load_heads(&mut self) -> Result<()> {
        self.heads.clear();
        self.heads_tbl.seek(SeekFrom::Start(0))?;
        loop {
            let start = self.heads_tbl.stream_position()?;
            let context_id = match self.heads_tbl.read_u64::<LittleEndian>() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(StoreError::Io(e)),
            };
            let head_turn_id = match self.heads_tbl.read_u64::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.heads_tbl.set_len(start)?;
                    break;
                }
            };
            let head_depth = match self.heads_tbl.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.heads_tbl.set_len(start)?;
                    break;
                }
            };
            let flags = match self.heads_tbl.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.heads_tbl.set_len(start)?;
                    break;
                }
            };
            let created_at_unix_ms = match self.heads_tbl.read_u64::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.heads_tbl.set_len(start)?;
                    break;
                }
            };
            let crc = match self.heads_tbl.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(_) => {
                    self.heads_tbl.set_len(start)?;
                    break;
                }
            };

            let mut buf = Vec::with_capacity(8 + 8 + 4 + 4 + 8);
            buf.write_u64::<LittleEndian>(context_id)?;
            buf.write_u64::<LittleEndian>(head_turn_id)?;
            buf.write_u32::<LittleEndian>(head_depth)?;
            buf.write_u32::<LittleEndian>(flags)?;
            buf.write_u64::<LittleEndian>(created_at_unix_ms)?;
            let mut hasher = Hasher::new();
            hasher.update(&buf);
            let actual_crc = hasher.finalize();
            if crc != actual_crc {
                self.heads_tbl.set_len(start)?;
                break;
            }

            self.heads.insert(
                context_id,
                ContextHead {
                    context_id,
                    head_turn_id,
                    head_depth,
                    created_at_unix_ms,
                    flags,
                },
            );
        }
        Ok(())
    }

    fn rebuild_index(&mut self) -> Result<()> {
        self.turns_idx.set_len(0)?;
        self.turns_idx.seek(SeekFrom::Start(0))?;
        for (turn_id, offset) in self.turn_index.iter() {
            self.turns_idx.write_u64::<LittleEndian>(*turn_id)?;
            self.turns_idx.write_u64::<LittleEndian>(*offset)?;
        }
        self.turns_idx.flush()?;
        Ok(())
    }

    fn update_counters(&mut self) {
        if let Some(max_id) = self.turns.keys().max().cloned() {
            self.next_turn_id = max_id + 1;
        }
        if let Some(max_ctx) = self.heads.keys().max().cloned() {
            self.next_context_id = max_ctx + 1;
        }
    }

    fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    pub fn create_context(&mut self, base_turn_id: u64) -> Result<ContextHead> {
        let (head_turn_id, head_depth) = if base_turn_id == 0 {
            (0, 0)
        } else {
            let turn = self
                .turns
                .get(&base_turn_id)
                .ok_or_else(|| StoreError::NotFound("base turn".into()))?;
            (turn.turn_id, turn.depth)
        };

        let context_id = self.next_context_id;
        self.next_context_id += 1;

        let head = ContextHead {
            context_id,
            head_turn_id,
            head_depth,
            created_at_unix_ms: Self::now_unix_ms(),
            flags: 0,
        };

        self.write_head(&head)?;
        self.heads.insert(context_id, head.clone());
        Ok(head)
    }

    pub fn fork_context(&mut self, base_turn_id: u64) -> Result<ContextHead> {
        self.create_context(base_turn_id)
    }

    pub fn get_head(&self, context_id: u64) -> Result<ContextHead> {
        self.heads
            .get(&context_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound("context".into()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_turn(
        &mut self,
        context_id: u64,
        parent_turn_id: u64,
        payload_hash: [u8; 32],
        encoding: u32,
        declared_type_id: String,
        declared_type_version: u32,
        compression: u32,
        uncompressed_len: u32,
    ) -> Result<TurnRecord> {
        let (parent_id, depth) = if parent_turn_id != 0 {
            let parent = self
                .turns
                .get(&parent_turn_id)
                .ok_or_else(|| StoreError::NotFound("parent turn".into()))?;
            (parent.turn_id, parent.depth + 1)
        } else {
            let head = self
                .heads
                .get(&context_id)
                .ok_or_else(|| StoreError::NotFound("context".into()))?;
            if head.head_turn_id == 0 {
                (0, 0)
            } else {
                let parent = self
                    .turns
                    .get(&head.head_turn_id)
                    .ok_or_else(|| StoreError::NotFound("head turn".into()))?;
                (parent.turn_id, parent.depth + 1)
            }
        };

        let turn_id = self.next_turn_id;
        self.next_turn_id += 1;

        let record = TurnRecord {
            turn_id,
            parent_turn_id: parent_id,
            depth,
            codec: encoding,
            type_tag: 0,
            payload_hash,
            flags: 0,
            created_at_unix_ms: Self::now_unix_ms(),
        };

        let offset = self.turns_log.seek(SeekFrom::End(0))?;
        let bytes = encode_turn_record(&record)?;
        self.turns_log.write_all(&bytes)?;
        self.turns_log.flush()?;

        self.turns_idx.seek(SeekFrom::End(0))?;
        self.turns_idx.write_u64::<LittleEndian>(turn_id)?;
        self.turns_idx.write_u64::<LittleEndian>(offset)?;
        self.turns_idx.flush()?;

        // store meta
        let mut meta_bytes = Vec::new();
        meta_bytes.write_u64::<LittleEndian>(turn_id)?;
        meta_bytes.write_u32::<LittleEndian>(declared_type_id.len() as u32)?;
        meta_bytes.extend_from_slice(declared_type_id.as_bytes());
        meta_bytes.write_u32::<LittleEndian>(declared_type_version)?;
        meta_bytes.write_u32::<LittleEndian>(encoding)?;
        meta_bytes.write_u32::<LittleEndian>(compression)?;
        meta_bytes.write_u32::<LittleEndian>(uncompressed_len)?;
        self.turns_meta.seek(SeekFrom::End(0))?;
        self.turns_meta.write_all(&meta_bytes)?;
        self.turns_meta.flush()?;

        self.turn_meta.insert(
            turn_id,
            TurnMeta {
                declared_type_id,
                declared_type_version,
                encoding,
                compression,
                uncompressed_len,
            },
        );
        self.turns.insert(turn_id, record.clone());
        self.turn_index.insert(turn_id, offset);

        // update head
        let head = ContextHead {
            context_id,
            head_turn_id: turn_id,
            head_depth: depth,
            created_at_unix_ms: record.created_at_unix_ms,
            flags: 0,
        };
        self.write_head(&head)?;
        self.heads.insert(context_id, head);

        Ok(record)
    }

    fn write_head(&mut self, head: &ContextHead) -> Result<()> {
        let mut buf = Vec::with_capacity(8 + 8 + 4 + 4 + 8 + 4);
        buf.write_u64::<LittleEndian>(head.context_id)?;
        buf.write_u64::<LittleEndian>(head.head_turn_id)?;
        buf.write_u32::<LittleEndian>(head.head_depth)?;
        buf.write_u32::<LittleEndian>(head.flags)?;
        buf.write_u64::<LittleEndian>(head.created_at_unix_ms)?;
        let mut hasher = Hasher::new();
        hasher.update(&buf);
        let crc = hasher.finalize();
        buf.write_u32::<LittleEndian>(crc)?;
        self.heads_tbl.seek(SeekFrom::End(0))?;
        self.heads_tbl.write_all(&buf)?;
        self.heads_tbl.flush()?;
        Ok(())
    }

    pub fn get_turn(&self, turn_id: u64) -> Result<TurnRecord> {
        self.turns
            .get(&turn_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound("turn".into()))
    }

    pub fn get_turn_meta(&self, turn_id: u64) -> Result<TurnMeta> {
        self.turn_meta
            .get(&turn_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound("turn meta".into()))
    }

    pub fn get_last(&self, context_id: u64, limit: u32) -> Result<Vec<TurnRecord>> {
        let head = self
            .heads
            .get(&context_id)
            .ok_or_else(|| StoreError::NotFound("context".into()))?;

        let mut results = Vec::new();
        let mut current = head.head_turn_id;
        while current != 0 && results.len() < limit as usize {
            let rec = self
                .turns
                .get(&current)
                .ok_or_else(|| StoreError::NotFound("turn".into()))?
                .clone();
            results.push(rec.clone());
            current = rec.parent_turn_id;
        }
        results.reverse();
        Ok(results)
    }

    pub fn get_before(
        &self,
        context_id: u64,
        before_turn_id: u64,
        limit: u32,
    ) -> Result<Vec<TurnRecord>> {
        let head = self
            .heads
            .get(&context_id)
            .ok_or_else(|| StoreError::NotFound("context".into()))?;

        if before_turn_id == 0 || head.head_turn_id == 0 {
            return self.get_last(context_id, limit);
        }

        let before = self
            .turns
            .get(&before_turn_id)
            .ok_or_else(|| StoreError::NotFound("before turn".into()))?;
        let mut current = before.parent_turn_id;
        let mut results = Vec::new();
        while current != 0 && results.len() < limit as usize {
            let rec = self
                .turns
                .get(&current)
                .ok_or_else(|| StoreError::NotFound("turn".into()))?
                .clone();
            results.push(rec.clone());
            current = rec.parent_turn_id;
        }
        results.reverse();
        Ok(results)
    }

    /// Get the first turn (depth=0) of a context, if it exists.
    pub fn get_first_turn(&self, context_id: u64) -> Result<TurnRecord> {
        let head = self
            .heads
            .get(&context_id)
            .ok_or_else(|| StoreError::NotFound("context".into()))?;

        // Walk back from head to find the turn with depth=0
        let mut current = head.head_turn_id;
        while current != 0 {
            let rec = self
                .turns
                .get(&current)
                .ok_or_else(|| StoreError::NotFound("turn".into()))?;
            if rec.depth == 0 {
                return Ok(rec.clone());
            }
            current = rec.parent_turn_id;
        }

        Err(StoreError::NotFound("first turn".into()))
    }

    pub fn list_recent_contexts(&self, limit: u32) -> Vec<ContextHead> {
        let mut contexts: Vec<ContextHead> = self.heads.values().cloned().collect();
        // Sort by created_at descending (most recent first)
        contexts.sort_by(|a, b| b.created_at_unix_ms.cmp(&a.created_at_unix_ms));
        contexts.truncate(limit as usize);
        contexts
    }
}

#[derive(Debug, Clone)]
pub struct TurnStoreStats {
    pub turns_total: usize,
    pub contexts_total: usize,
    pub heads_total: usize,
    pub turns_log_bytes: u64,
    pub turns_index_bytes: u64,
    pub turns_meta_bytes: u64,
    pub heads_table_bytes: u64,
}

fn file_len(path: &std::path::PathBuf) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn encode_turn_record(record: &TurnRecord) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(80);
    buf.write_u64::<LittleEndian>(record.turn_id)?;
    buf.write_u64::<LittleEndian>(record.parent_turn_id)?;
    buf.write_u32::<LittleEndian>(record.depth)?;
    buf.write_u32::<LittleEndian>(record.codec)?;
    buf.write_u64::<LittleEndian>(record.type_tag)?;
    buf.extend_from_slice(&record.payload_hash);
    buf.write_u32::<LittleEndian>(record.flags)?;
    buf.write_u64::<LittleEndian>(record.created_at_unix_ms)?;
    let mut hasher = Hasher::new();
    hasher.update(&buf);
    let crc = hasher.finalize();
    buf.write_u32::<LittleEndian>(crc)?;
    Ok(buf)
}

fn read_turn_record(reader: &mut File) -> Result<TurnRecord> {
    let turn_id = reader.read_u64::<LittleEndian>()?;
    let parent_turn_id = reader.read_u64::<LittleEndian>()?;
    let depth = reader.read_u32::<LittleEndian>()?;
    let codec = reader.read_u32::<LittleEndian>()?;
    let type_tag = reader.read_u64::<LittleEndian>()?;
    let mut payload_hash = [0u8; 32];
    reader.read_exact(&mut payload_hash)?;
    let flags = reader.read_u32::<LittleEndian>()?;
    let created_at_unix_ms = reader.read_u64::<LittleEndian>()?;
    let crc = reader.read_u32::<LittleEndian>()?;

    let mut buf = Vec::with_capacity(80);
    buf.write_u64::<LittleEndian>(turn_id)?;
    buf.write_u64::<LittleEndian>(parent_turn_id)?;
    buf.write_u32::<LittleEndian>(depth)?;
    buf.write_u32::<LittleEndian>(codec)?;
    buf.write_u64::<LittleEndian>(type_tag)?;
    buf.extend_from_slice(&payload_hash);
    buf.write_u32::<LittleEndian>(flags)?;
    buf.write_u64::<LittleEndian>(created_at_unix_ms)?;
    let mut hasher = Hasher::new();
    hasher.update(&buf);
    let actual_crc = hasher.finalize();

    if crc != actual_crc {
        return Err(StoreError::Corrupt("turn crc mismatch".into()));
    }

    Ok(TurnRecord {
        turn_id,
        parent_turn_id,
        depth,
        codec,
        type_tag,
        payload_hash,
        flags,
        created_at_unix_ms,
    })
}
