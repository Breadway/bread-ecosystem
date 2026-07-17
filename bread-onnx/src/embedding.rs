//! Shared BERT-family embedding pipeline: tokenize → build `input_ids`/
//! `attention_mask`/`token_type_ids` tensors → run → mean-pool the
//! non-padded positions of `last_hidden_state` → L2-normalize → clamp/pad to
//! a configured output dimension.
//!
//! This is extracted from two independently-written but essentially
//! byte-identical implementations:
//! - `breadarrd/src/matcher/embed.rs::OrtEmbedder::embed` (lines 45-111) and
//!   its `l2_normalize` (lines 114-121)
//! - `breadmill/src/embed.rs::OrtEmbedder::embed_with_prefix` (lines 65-153)
//!   and its `l2_normalize` (lines 156-163)
//!
//! Both truncate to a max sequence length, build the same three `i64`
//! tensors, run the same `input_ids`/`attention_mask`/`token_type_ids` →
//! `last_hidden_state` shape contract, mean-pool over `actual_seq.min(mask.len())`
//! positions (both already independently arrived at the same `.min()` guard
//! for execution providers that pad the output sequence dimension), and
//! L2-normalize with the same `1e-10` epsilon. `breadmill`'s only real
//! difference is prepending a document/query prefix string before
//! tokenizing, which stays the caller's responsibility here — pass the
//! already-prefixed text to [`EmbeddingSession::embed`].

use std::path::Path;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::provider::Provider;
use crate::session::build_session;

pub struct EmbeddingSession {
    session: Session,
    tokenizer: Tokenizer,
    dim: usize,
    max_seq_len: usize,
}

impl EmbeddingSession {
    /// Load a BERT-family embedding model + tokenizer, selecting execution
    /// providers via [`build_session`]. `dim` is the output embedding
    /// dimension (results are truncated/zero-padded to it — matches how
    /// both original implementations handled a model whose `dim` config
    /// might not exactly match `last_hidden_state`'s actual width). `max_seq_len`
    /// caps tokenized input length before inference (truncating, not
    /// erroring) to bound attention memory on pathological inputs.
    pub fn load(
        model_path: &Path,
        tokenizer_path: &Path,
        dim: usize,
        max_seq_len: usize,
        providers: &[Provider],
    ) -> anyhow::Result<Self> {
        let session = build_session(model_path, GraphOptimizationLevel::Level3, providers)?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("failed to load tokenizer: {e}"))?;
        Ok(Self { session, tokenizer, dim, max_seq_len })
    }

    /// Embed `text` (already prefixed by the caller, if the model expects a
    /// document/query prefix). Returns an L2-normalized vector of length
    /// `dim`.
    pub fn embed(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenization failed: {e}"))?;

        let mut ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        let mut mask: Vec<i64> = encoding.get_attention_mask().iter().map(|&x| x as i64).collect();
        let mut type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&x| x as i64).collect();

        ids.truncate(self.max_seq_len);
        mask.truncate(self.max_seq_len);
        type_ids.truncate(self.max_seq_len);

        let seq_len = ids.len() as i64;
        let id_tensor = Tensor::<i64>::from_array((vec![1i64, seq_len], ids))
            .map_err(|e| anyhow::anyhow!("failed to build input_ids tensor: {e}"))?;
        let mask_tensor = Tensor::<i64>::from_array((vec![1i64, seq_len], mask.clone()))
            .map_err(|e| anyhow::anyhow!("failed to build attention_mask tensor: {e}"))?;
        let type_tensor = Tensor::<i64>::from_array((vec![1i64, seq_len], type_ids))
            .map_err(|e| anyhow::anyhow!("failed to build token_type_ids tensor: {e}"))?;

        let outputs = self
            .session
            .run(ort::inputs! {
                "input_ids" => id_tensor,
                "attention_mask" => mask_tensor,
                "token_type_ids" => type_tensor,
            })
            .map_err(|e| anyhow::anyhow!("ort inference failed: {e}"))?;

        let (shape, data) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("failed to extract last_hidden_state: {e}"))?;

        let actual_seq = shape[1] as usize;
        let actual_dim = shape[2] as usize;

        Ok(mean_pool_normalize(data, &mask, actual_seq, actual_dim, self.dim))
    }
}

/// Mean-pool `data` (flattened `[1, actual_seq, actual_dim]`) over the
/// positions `mask` marks as non-padding, L2-normalize the result, then
/// clamp/zero-pad to `target_dim`. `actual_seq.min(mask.len())` guards
/// against execution providers (MIGraphX observed doing this) that pad the
/// output sequence dimension for kernel efficiency, making `actual_seq`
/// exceed the caller's own `mask` length.
fn mean_pool_normalize(data: &[f32], mask: &[i64], actual_seq: usize, actual_dim: usize, target_dim: usize) -> Vec<f32> {
    let mut result = vec![0.0f32; actual_dim];
    let mut count = 0usize;
    for t in 0..actual_seq.min(mask.len()) {
        if mask[t] > 0 {
            for d in 0..actual_dim {
                result[d] += data[t * actual_dim + d];
            }
            count += 1;
        }
    }
    if count > 0 {
        for x in &mut result {
            *x /= count as f32;
        }
    }

    l2_normalize(&mut result);
    result.truncate(target_dim);
    while result.len() < target_dim {
        result.push(0.0);
    }
    result
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_normalize_produces_unit_vector() {
        let mut v = vec![3.0, 4.0];
        l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_leaves_zero_vector_untouched() {
        let mut v = vec![0.0, 0.0, 0.0];
        l2_normalize(&mut v);
        assert_eq!(v, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn cosine_similarity_of_identical_unit_vectors_is_one() {
        let mut v = vec![1.0, 2.0, 3.0];
        l2_normalize(&mut v);
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_orthogonal_vectors_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn mean_pool_ignores_padded_positions() {
        // actual_dim = 2, 3 positions: two real tokens + one padded (mask=0)
        let data = vec![
            1.0, 1.0, // t0: real
            9.0, 9.0, // t1: padded, should be ignored
            3.0, 3.0, // t2: real
        ];
        let mask = vec![1, 0, 1];
        let pooled = mean_pool_normalize(&data, &mask, 3, 2, 2);
        // Mean of (1,1) and (3,3) is (2,2), normalized to unit length.
        let expected_norm = (2.0f32 * 2.0 + 2.0 * 2.0).sqrt();
        assert!((pooled[0] - 2.0 / expected_norm).abs() < 1e-5);
        assert!((pooled[1] - 2.0 / expected_norm).abs() < 1e-5);
    }

    #[test]
    fn mean_pool_clamps_actual_seq_to_mask_len_for_padded_ep_output() {
        // Regression guard for the MIGraphX-padded-output-sequence case both
        // original implementations independently guarded against: actual_seq
        // (4) exceeds mask.len() (2) — must not index out of the mask.
        let data = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let mask = vec![1, 1];
        let pooled = mean_pool_normalize(&data, &mask, 4, 2, 2);
        assert!(pooled.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn mean_pool_pads_short_result_to_target_dim() {
        let data = vec![1.0, 1.0];
        let mask = vec![1];
        let pooled = mean_pool_normalize(&data, &mask, 1, 1, 4);
        assert_eq!(pooled.len(), 4);
        assert_eq!(pooled[2], 0.0);
        assert_eq!(pooled[3], 0.0);
    }
}
