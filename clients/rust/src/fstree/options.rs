// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

#![allow(clippy::type_complexity)]

use std::path::Path;
use std::sync::Arc;

use glob::Pattern;

pub type SnapshotOption = Arc<dyn Fn(&mut Options) + Send + Sync>;

#[derive(Clone)]
pub struct Options {
    pub exclude_patterns: Vec<String>,
    pub exclude_fn: std::option::Option<Arc<dyn Fn(&str, bool) -> bool + Send + Sync>>,
    pub follow_symlinks: bool,
    pub max_file_size: i64,
    pub max_files: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            exclude_patterns: Vec::new(),
            exclude_fn: None,
            follow_symlinks: false,
            max_file_size: 100 * 1024 * 1024,
            max_files: 100_000,
        }
    }
}

pub fn with_exclude(patterns: impl IntoIterator<Item = impl Into<String>>) -> SnapshotOption {
    let patterns: Vec<String> = patterns.into_iter().map(|p| p.into()).collect();
    Arc::new(move |opts| {
        opts.exclude_patterns.extend(patterns.clone());
    })
}

pub fn with_exclude_func<F>(func: F) -> SnapshotOption
where
    F: Fn(&str, bool) -> bool + Send + Sync + 'static,
{
    let func = Arc::new(func);
    Arc::new(move |opts| opts.exclude_fn = Some(func.clone()))
}

pub fn with_follow_symlinks() -> SnapshotOption {
    Arc::new(|opts| opts.follow_symlinks = true)
}

pub fn with_max_file_size(bytes: i64) -> SnapshotOption {
    Arc::new(move |opts| opts.max_file_size = bytes)
}

pub fn with_max_files(count: usize) -> SnapshotOption {
    Arc::new(move |opts| opts.max_files = count)
}

impl Options {
    pub fn should_exclude(&self, rel_path: &str, is_dir: bool) -> bool {
        if let Some(func) = &self.exclude_fn {
            if func(rel_path, is_dir) {
                return true;
            }
        }

        let rel_path = normalize_path(rel_path);
        let basename = Path::new(&rel_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        for pattern in &self.exclude_patterns {
            if is_double_star_dir(pattern, &rel_path, is_dir) {
                return true;
            }
            if matches_glob(pattern, &rel_path) {
                return true;
            }
            if matches_glob(pattern, basename) {
                return true;
            }
        }
        false
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn matches_glob(pattern: &str, path: &str) -> bool {
    Pattern::new(pattern)
        .map(|p| p.matches(path))
        .unwrap_or(false)
}

fn is_double_star_dir(pattern: &str, rel_path: &str, is_dir: bool) -> bool {
    if !is_dir {
        return false;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        if rel_path == prefix {
            return true;
        }
        let prefix_with_sep = format!("{prefix}/");
        return rel_path.starts_with(&prefix_with_sep) || matches_glob(prefix, rel_path);
    }
    false
}
