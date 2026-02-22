// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

//! HNSW vector index for cosine similarity search over turn embeddings.
//!
//! Embeddings are stored as content-addressed blobs (BLAKE3 hash in `TurnRecord.embedding_hash`).
//! This module provides an in-memory HNSW index that maps turn IDs to their embedding vectors
//! and supports approximate nearest-neighbor queries using cosine distance.

use instant_distance::Point;
use ordered_float::OrderedFloat;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// EmbeddingPoint — wrapper around Vec<f32> implementing instant_distance::Point
// ---------------------------------------------------------------------------

/// A dense embedding vector that implements cosine distance for HNSW indexing.
#[derive(Clone, Debug)]
pub struct EmbeddingPoint {
    pub values: Vec<f32>,
    norm: f32,
}

impl EmbeddingPoint {
    /// Create a new embedding point, pre-computing the L2 norm for fast cosine distance.
    pub fn new(values: Vec<f32>) -> Self {
        let norm = dot(&values, &values).sqrt();
        Self { values, norm }
    }

    /// Dimensionality of the embedding.
    pub fn dim(&self) -> usize {
        self.values.len()
    }
}

impl Point for EmbeddingPoint {
    /// Cosine distance: `1.0 - cosine_similarity`.
    ///
    /// Returns 0.0 for identical directions, 1.0 for orthogonal vectors,
    /// and 2.0 for opposite directions.
    fn distance(&self, other: &Self) -> f32 {
        if self.norm == 0.0 || other.norm == 0.0 {
            return 1.0; // treat zero vectors as maximally dissimilar
        }
        let similarity = dot(&self.values, &other.values) / (self.norm * other.norm);
        // Clamp to [-1, 1] to handle floating-point drift
        1.0 - similarity.clamp(-1.0, 1.0)
    }
}

/// Dot product of two slices. Panics if lengths differ.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "dot product requires equal dimensions");
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

// ---------------------------------------------------------------------------
// VectorIndex — manages the HNSW index with incremental inserts
// ---------------------------------------------------------------------------

/// In-memory HNSW index mapping turn IDs to embedding vectors.
///
/// Points are accumulated in a buffer. The HNSW index is (re)built lazily
/// on the first `search` call after any `insert`. This amortises the cost
/// of batch inserts during startup while keeping the API simple.
pub struct VectorIndex {
    /// All known embeddings, keyed by turn_id.
    points: HashMap<u64, EmbeddingPoint>,
    /// Ordered list of (turn_id, point) used to build the HNSW index.
    /// Indices here correspond to `PointId` ordinals in the built HNSW.
    ordered: Vec<(u64, EmbeddingPoint)>,
    /// The built HNSW index. `None` if dirty (inserts since last build).
    hnsw: Option<instant_distance::Hnsw<EmbeddingPoint>>,
}

impl VectorIndex {
    /// Create an empty vector index.
    pub fn new() -> Self {
        Self {
            points: HashMap::new(),
            ordered: Vec::new(),
            hnsw: None,
        }
    }

    /// Insert an embedding for a turn. Replaces any previous embedding for the same turn.
    ///
    /// The HNSW index is invalidated and will be rebuilt on the next `search`.
    pub fn insert(&mut self, turn_id: u64, embedding: Vec<f32>) {
        let point = EmbeddingPoint::new(embedding);
        self.points.insert(turn_id, point);
        // Mark index as dirty — will be rebuilt on next search
        self.hnsw = None;
    }

    /// Search for the nearest neighbors of `query`, returning up to `limit` results
    /// with similarity score >= `min_score`.
    ///
    /// Returns `(turn_id, similarity_score)` pairs sorted by descending similarity.
    /// Similarity is `1.0 - cosine_distance`, so 1.0 = identical, 0.0 = orthogonal.
    pub fn search(&mut self, query: &[f32], limit: usize, min_score: f32) -> Vec<(u64, f32)> {
        if self.points.is_empty() || limit == 0 {
            return Vec::new();
        }

        self.ensure_built();

        let hnsw = self.hnsw.as_ref().unwrap();
        let query_point = EmbeddingPoint::new(query.to_vec());
        let mut search = instant_distance::Search::default();

        let results: Vec<(u64, f32)> = hnsw
            .search(&query_point, &mut search)
            .filter_map(|item| {
                let idx = item.pid.into_inner();
                let (turn_id, _) = &self.ordered[idx as usize];
                let similarity = 1.0 - item.distance;
                if similarity >= min_score {
                    Some((*turn_id, similarity))
                } else {
                    None
                }
            })
            .take(limit)
            .collect();

        // Sort by similarity descending (stable for equal scores)
        let mut sorted = results;
        sorted.sort_by(|a, b| OrderedFloat(b.1).cmp(&OrderedFloat(a.1)));
        sorted
    }

    /// Number of embeddings in the index.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Rebuild the HNSW index from the current point set if it has been invalidated.
    fn ensure_built(&mut self) {
        if self.hnsw.is_some() {
            return;
        }

        // Rebuild ordered list from the HashMap
        self.ordered = self
            .points
            .iter()
            .map(|(tid, pt)| (*tid, pt.clone()))
            .collect();

        let hnsw_points: Vec<EmbeddingPoint> =
            self.ordered.iter().map(|(_, pt)| pt.clone()).collect();

        if hnsw_points.is_empty() {
            return;
        }

        let (hnsw, _point_ids) = instant_distance::Builder::default().build_hnsw(hnsw_points);
        self.hnsw = Some(hnsw);
    }
}

impl Default for VectorIndex {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_distance_identical_vectors() {
        let a = EmbeddingPoint::new(vec![1.0, 0.0, 0.0]);
        let b = EmbeddingPoint::new(vec![1.0, 0.0, 0.0]);
        let dist = a.distance(&b);
        assert!(
            dist.abs() < 1e-6,
            "identical vectors should have distance ~0, got {dist}"
        );
    }

    #[test]
    fn cosine_distance_orthogonal_vectors() {
        let a = EmbeddingPoint::new(vec![1.0, 0.0, 0.0]);
        let b = EmbeddingPoint::new(vec![0.0, 1.0, 0.0]);
        let dist = a.distance(&b);
        assert!(
            (dist - 1.0).abs() < 1e-6,
            "orthogonal vectors should have distance ~1.0, got {dist}"
        );
    }

    #[test]
    fn cosine_distance_opposite_vectors() {
        let a = EmbeddingPoint::new(vec![1.0, 0.0, 0.0]);
        let b = EmbeddingPoint::new(vec![-1.0, 0.0, 0.0]);
        let dist = a.distance(&b);
        assert!(
            (dist - 2.0).abs() < 1e-6,
            "opposite vectors should have distance ~2.0, got {dist}"
        );
    }

    #[test]
    fn cosine_distance_zero_vector_returns_one() {
        let a = EmbeddingPoint::new(vec![1.0, 2.0, 3.0]);
        let zero = EmbeddingPoint::new(vec![0.0, 0.0, 0.0]);
        assert!(
            (a.distance(&zero) - 1.0).abs() < 1e-6,
            "zero vector should yield distance 1.0"
        );
        assert!(
            (zero.distance(&a) - 1.0).abs() < 1e-6,
            "zero vector should yield distance 1.0"
        );
    }

    #[test]
    fn cosine_distance_scaled_vectors() {
        let a = EmbeddingPoint::new(vec![1.0, 2.0, 3.0]);
        let b = EmbeddingPoint::new(vec![2.0, 4.0, 6.0]);
        let dist = a.distance(&b);
        assert!(
            dist.abs() < 1e-5,
            "parallel vectors (different magnitude) should have distance ~0, got {dist}"
        );
    }

    #[test]
    fn vector_index_insert_and_search() {
        let mut index = VectorIndex::new();
        index.insert(1, vec![1.0, 0.0, 0.0]);
        index.insert(2, vec![0.0, 1.0, 0.0]);
        index.insert(3, vec![0.9, 0.1, 0.0]); // close to turn 1

        assert_eq!(index.len(), 3);

        let results = index.search(&[1.0, 0.0, 0.0], 2, 0.0);
        assert!(!results.is_empty(), "search should return results");

        // The closest match to [1,0,0] should be turn 1 (exact match)
        assert_eq!(
            results[0].0, 1,
            "first result should be turn 1 (exact match)"
        );
        assert!(
            (results[0].1 - 1.0).abs() < 1e-5,
            "exact match should have similarity ~1.0"
        );
    }

    #[test]
    fn vector_index_min_score_filter() {
        let mut index = VectorIndex::new();
        index.insert(1, vec![1.0, 0.0, 0.0]);
        index.insert(2, vec![0.0, 1.0, 0.0]); // orthogonal, similarity ~0

        // With high min_score, orthogonal vector should be filtered out
        let results = index.search(&[1.0, 0.0, 0.0], 10, 0.5);
        for (tid, score) in &results {
            assert!(
                *score >= 0.5,
                "turn {tid} has score {score} which is below min_score 0.5"
            );
        }
    }

    #[test]
    fn vector_index_empty_search() {
        let mut index = VectorIndex::new();
        let results = index.search(&[1.0, 0.0, 0.0], 10, 0.0);
        assert!(results.is_empty(), "empty index should return no results");
    }

    #[test]
    fn vector_index_replace_embedding() {
        let mut index = VectorIndex::new();
        index.insert(1, vec![1.0, 0.0, 0.0]);
        index.insert(1, vec![0.0, 1.0, 0.0]); // replace

        assert_eq!(index.len(), 1, "replacing should not increase count");

        let results = index.search(&[0.0, 1.0, 0.0], 1, 0.0);
        assert_eq!(
            results[0].0, 1,
            "should find the replaced embedding"
        );
        assert!(
            (results[0].1 - 1.0).abs() < 1e-5,
            "replaced embedding should match query exactly"
        );
    }
}
