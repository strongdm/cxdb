// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use blake3::Hasher;

use crate::encoding::encode_msgpack;

use super::options::{Options, SnapshotOption};
use super::types::{
    EntryKindDirectory, EntryKindFile, EntryKindSymlink, FileRef, Snapshot, SnapshotStats,
    TreeEntry,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FstreeErrorKind {
    TooManyFiles,
    FileTooLarge,
    CyclicLink,
    Io,
    Msgpack,
    Client,
    Other,
}

#[derive(Debug)]
pub struct FstreeError {
    pub kind: FstreeErrorKind,
    pub detail: String,
}

impl std::fmt::Display for FstreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fstree: {}", self.detail)
    }
}

impl std::error::Error for FstreeError {}

impl FstreeError {
    pub(crate) fn new(kind: FstreeErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

#[allow(non_upper_case_globals)]
pub const ErrTooManyFiles: FstreeErrorKind = FstreeErrorKind::TooManyFiles;
#[allow(non_upper_case_globals)]
pub const ErrFileTooLarge: FstreeErrorKind = FstreeErrorKind::FileTooLarge;
#[allow(non_upper_case_globals)]
pub const ErrCyclicLink: FstreeErrorKind = FstreeErrorKind::CyclicLink;

pub type Result<T> = std::result::Result<T, FstreeError>;

pub fn capture(
    root: impl AsRef<Path>,
    opts: impl IntoIterator<Item = SnapshotOption>,
) -> Result<Snapshot> {
    let start = SystemTime::now();
    let abs_root = fs::canonicalize(root.as_ref())
        .map_err(|err| FstreeError::new(FstreeErrorKind::Io, err.to_string()))?;

    let metadata = fs::metadata(&abs_root)
        .map_err(|err| FstreeError::new(FstreeErrorKind::Io, err.to_string()))?;
    if !metadata.is_dir() {
        return Err(FstreeError::new(
            FstreeErrorKind::Other,
            format!("root is not a directory: {}", abs_root.display()),
        ));
    }

    let mut options = Options::default();
    for opt in opts {
        opt(&mut options);
    }

    let mut builder = Builder::new(options);
    let root_hash = builder.build_tree(&abs_root, Path::new(""))?;

    Ok(Snapshot {
        root_hash,
        trees: builder.trees,
        files: builder.files,
        symlinks: builder.symlinks,
        captured_at: start,
        stats: SnapshotStats {
            file_count: builder.file_count,
            dir_count: builder.dir_count,
            symlink_count: builder.symlink_count,
            total_bytes: builder.total_bytes,
            duration: start.elapsed().unwrap_or(Duration::from_secs(0)),
        },
    })
}

pub fn deserialize_tree(data: &[u8]) -> Result<Vec<TreeEntry>> {
    crate::encoding::decode_msgpack_into(data)
        .map_err(|err| FstreeError::new(FstreeErrorKind::Msgpack, err.to_string()))
}

struct Builder {
    options: Options,
    trees: HashMap<[u8; 32], Vec<u8>>,
    files: HashMap<[u8; 32], FileRef>,
    symlinks: HashMap<[u8; 32], String>,
    visited: HashSet<PathBuf>,
    file_count: usize,
    dir_count: usize,
    symlink_count: usize,
    total_bytes: u64,
}

impl Builder {
    fn new(options: Options) -> Self {
        Self {
            options,
            trees: HashMap::new(),
            files: HashMap::new(),
            symlinks: HashMap::new(),
            visited: HashSet::new(),
            file_count: 0,
            dir_count: 0,
            symlink_count: 0,
            total_bytes: 0,
        }
    }

    fn build_tree(&mut self, abs_path: &Path, rel_path: &Path) -> Result<[u8; 32]> {
        if let Ok(real_path) = fs::canonicalize(abs_path) {
            if self.visited.contains(&real_path) {
                return Err(FstreeError::new(
                    FstreeErrorKind::CyclicLink,
                    "cyclic symbolic link detected",
                ));
            }
            self.visited.insert(real_path.clone());
        }

        let mut entries = Vec::new();
        let dir_entries = fs::read_dir(abs_path)
            .map_err(|err| FstreeError::new(FstreeErrorKind::Io, err.to_string()))?;

        for entry in dir_entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy().to_string();
            let child_rel = rel_path.join(&name);
            let child_abs = abs_path.join(&name);
            let rel_str = child_rel.to_string_lossy();

            if self.options.should_exclude(
                &rel_str,
                entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
            ) {
                continue;
            }

            let metadata = if self.options.follow_symlinks {
                fs::metadata(&child_abs)
            } else {
                fs::symlink_metadata(&child_abs)
            };
            let metadata = match metadata {
                Ok(meta) => meta,
                Err(_) => continue,
            };

            match self.build_entry(&child_abs, &child_rel, &name, &metadata) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    if err.kind == FstreeErrorKind::TooManyFiles
                        || err.kind == FstreeErrorKind::CyclicLink
                    {
                        return Err(err);
                    }
                    // Skip individual file errors
                    continue;
                }
            }
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let tree_bytes = encode_msgpack(&entries)
            .map_err(|err| FstreeError::new(FstreeErrorKind::Msgpack, err.to_string()))?;
        let hash = blake3::hash(&tree_bytes);
        self.trees.insert(*hash.as_bytes(), tree_bytes);
        self.dir_count += 1;

        if let Ok(real_path) = fs::canonicalize(abs_path) {
            self.visited.remove(&real_path);
        }

        Ok(*hash.as_bytes())
    }

    fn build_entry(
        &mut self,
        abs_path: &Path,
        rel_path: &Path,
        name: &str,
        metadata: &fs::Metadata,
    ) -> Result<TreeEntry> {
        let mode = metadata.permissions().perm_mode() & 0o7777;

        if metadata.file_type().is_symlink() && !self.options.follow_symlinks {
            let target = fs::read_link(abs_path)
                .map_err(|err| FstreeError::new(FstreeErrorKind::Io, err.to_string()))?;
            let target_str = target.to_string_lossy().to_string();
            let hash = blake3::hash(target_str.as_bytes());
            self.symlink_count += 1;
            self.symlinks.insert(*hash.as_bytes(), target_str.clone());
            return Ok(TreeEntry {
                name: name.to_string(),
                kind: EntryKindSymlink,
                mode,
                size: target_str.len() as u64,
                hash: *hash.as_bytes(),
            });
        }

        if metadata.is_dir() {
            let dir_hash = self.build_tree(abs_path, rel_path)?;
            return Ok(TreeEntry {
                name: name.to_string(),
                kind: EntryKindDirectory,
                mode,
                size: 0,
                hash: dir_hash,
            });
        }

        if self.file_count >= self.options.max_files {
            return Err(FstreeError::new(
                FstreeErrorKind::TooManyFiles,
                "too many files",
            ));
        }

        let size = metadata.len();
        if size as i64 > self.options.max_file_size {
            return Err(FstreeError::new(
                FstreeErrorKind::FileTooLarge,
                format!("file too large: {} ({} bytes)", rel_path.display(), size),
            ));
        }

        let hash = hash_file(abs_path)
            .map_err(|err| FstreeError::new(FstreeErrorKind::Io, err.to_string()))?;
        self.files.insert(
            hash,
            FileRef {
                path: abs_path.to_path_buf(),
                size,
                hash,
            },
        );
        self.file_count += 1;
        self.total_bytes += size;

        Ok(TreeEntry {
            name: name.to_string(),
            kind: EntryKindFile,
            mode,
            size,
            hash,
        })
    }
}

fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hash = hasher.finalize();
    Ok(*hash.as_bytes())
}

trait PermissionsExt {
    fn perm_mode(&self) -> u32;
}

impl PermissionsExt for fs::Permissions {
    #[cfg(unix)]
    fn perm_mode(&self) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        self.mode()
    }

    #[cfg(not(unix))]
    fn perm_mode(&self) -> u32 {
        0
    }
}
