// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use crate::client::RequestContext;
use crate::fs::PutBlobRequest;
use crate::Client;

use super::capture::{FstreeError, FstreeErrorKind, Result as FstreeResult};
use super::types::Snapshot;

#[derive(Debug, Clone, Default)]
pub struct UploadResult {
    pub root_hash: [u8; 32],
    pub trees_uploaded: usize,
    pub trees_skipped: usize,
    pub files_uploaded: usize,
    pub files_skipped: usize,
    pub bytes_uploaded: i64,
}

impl Snapshot {
    pub fn upload(&self, ctx: &RequestContext, client: &Client) -> FstreeResult<UploadResult> {
        let mut result = UploadResult {
            root_hash: self.root_hash,
            ..UploadResult::default()
        };

        for data in self.trees.values() {
            let was_new = upload_blob(ctx, client, data.to_vec())
                .map_err(|err| FstreeError::new(FstreeErrorKind::Client, err.to_string()))?;
            if was_new {
                result.trees_uploaded += 1;
                result.bytes_uploaded += data.len() as i64;
            } else {
                result.trees_skipped += 1;
            }
        }

        for file_ref in self.files.values() {
            let content = std::fs::read(&file_ref.path)
                .map_err(|err| FstreeError::new(FstreeErrorKind::Io, err.to_string()))?;
            let was_new = upload_blob(ctx, client, content.clone())
                .map_err(|err| FstreeError::new(FstreeErrorKind::Client, err.to_string()))?;
            if was_new {
                result.files_uploaded += 1;
                result.bytes_uploaded += content.len() as i64;
            } else {
                result.files_skipped += 1;
            }
        }

        for target in self.symlinks.values() {
            let bytes = target.as_bytes().to_vec();
            let was_new = upload_blob(ctx, client, bytes.clone())
                .map_err(|err| FstreeError::new(FstreeErrorKind::Client, err.to_string()))?;
            if was_new {
                result.files_uploaded += 1;
                result.bytes_uploaded += bytes.len() as i64;
            } else {
                result.files_skipped += 1;
            }
        }

        Ok(result)
    }
}

fn upload_blob(
    ctx: &RequestContext,
    client: &Client,
    data: Vec<u8>,
) -> Result<bool, crate::error::Error> {
    let result = client.put_blob(ctx, &PutBlobRequest { data })?;
    Ok(result.was_new)
}

pub fn upload_and_attach(
    ctx: &RequestContext,
    client: &Client,
    root: impl AsRef<std::path::Path>,
    turn_id: u64,
    opts: impl IntoIterator<Item = super::options::SnapshotOption>,
) -> FstreeResult<UploadResult> {
    let snapshot = super::capture::capture(root, opts)?;
    let result = snapshot.upload(ctx, client)?;
    client
        .attach_fs(
            ctx,
            &crate::fs::AttachFsRequest {
                turn_id,
                fs_root_hash: snapshot.root_hash,
            },
        )
        .map_err(|err| FstreeError::new(FstreeErrorKind::Client, err.to_string()))?;
    Ok(result)
}

pub fn capture_and_upload(
    ctx: &RequestContext,
    client: &Client,
    root: impl AsRef<std::path::Path>,
    opts: impl IntoIterator<Item = super::options::SnapshotOption>,
) -> FstreeResult<(super::types::Snapshot, UploadResult)> {
    let snapshot = super::capture::capture(root, opts)?;
    let result = snapshot.upload(ctx, client)?;
    Ok((snapshot, result))
}
