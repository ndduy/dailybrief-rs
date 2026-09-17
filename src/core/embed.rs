//! Local embeddings (ADR 0005): an `Embedder` trait, a deterministic fake for tests, and the
//! `fastembed` BGE-small-en-v1.5 implementation loaded lazily on first use. Callers run `embed`
//! inside `spawn_blocking`; nothing here is async.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};

use super::vector::{self, DIMENSIONS};
use crate::config::Config;

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding model: {0}")]
    Model(String),
    #[error("embedding model returned {got} dimensions, expected {expected}")]
    Dimension { expected: usize, got: usize },
}

/// Text → unit-length 384-d vectors, in input order.
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// SHA-256 of the text expanded into a deterministic unit vector. Different texts give different
/// (uncorrelated) vectors; the same text always gives the same one. Never loads a model.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeEmbedder;

impl FakeEmbedder {
    fn vector_for(text: &str) -> Vec<f32> {
        let seed = Sha256::digest(text.as_bytes());
        let mut out = Vec::with_capacity(DIMENSIONS);
        let mut counter: u32 = 0;
        while out.len() < DIMENSIONS {
            let mut h = Sha256::new();
            h.update(seed);
            h.update(counter.to_le_bytes());
            for b in h.finalize() {
                if out.len() == DIMENSIONS {
                    break;
                }
                out.push(f32::from(b) / 127.5 - 1.0);
            }
            counter += 1;
        }
        vector::normalize(&mut out);
        out
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| Self::vector_for(t)).collect())
    }
}

/// `fastembed` BGE-small-en-v1.5 with the model cached under `<data_dir>/models`.
/// Construction is free; the first `embed` downloads (once) and loads the model.
pub struct FastEmbedder {
    cache_dir: PathBuf,
    model: Mutex<Option<fastembed::TextEmbedding>>,
}

impl std::fmt::Debug for FastEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastEmbedder")
            .field("cache_dir", &self.cache_dir)
            .field("loaded", &self.is_loaded())
            .finish()
    }
}

impl FastEmbedder {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            model: Mutex::new(None),
        }
    }

    /// Whether the model has been loaded yet (tests assert construction is lazy).
    pub fn is_loaded(&self) -> bool {
        self.model
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn load(cache_dir: &Path) -> Result<fastembed::TextEmbedding, EmbedError> {
        let options = fastembed::TextInitOptions::new(fastembed::EmbeddingModel::BGESmallENV15)
            .with_cache_dir(cache_dir.to_path_buf())
            .with_show_download_progress(false);
        fastembed::TextEmbedding::try_new(options).map_err(|e| EmbedError::Model(e.to_string()))
    }
}

impl Embedder for FastEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = self.model.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(Self::load(&self.cache_dir)?);
        }
        let model = guard
            .as_mut()
            .ok_or_else(|| EmbedError::Model("model missing after load".to_string()))?;
        let mut vectors = model
            .embed(texts, None)
            .map_err(|e| EmbedError::Model(e.to_string()))?;
        for v in &mut vectors {
            if v.len() != DIMENSIONS {
                return Err(EmbedError::Dimension {
                    expected: DIMENSIONS,
                    got: v.len(),
                });
            }
            vector::normalize(v);
        }
        Ok(vectors)
    }
}

/// The production embedder for a config: models cached in the data directory.
pub fn embedder_for(config: &Config) -> Arc<dyn Embedder> {
    Arc::new(FastEmbedder::new(config.paths.data_dir.join("models")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: &str) -> String {
        t.to_string()
    }

    #[test]
    fn fake_is_deterministic_and_unit_length() {
        let a = FakeEmbedder.embed(&[s("Rust ownership")]).unwrap();
        let b = FakeEmbedder.embed(&[s("Rust ownership")]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].len(), DIMENSIONS);
        let norm: f32 = a[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm {norm}");
    }

    #[test]
    fn fake_differs_for_different_text() {
        let v = FakeEmbedder
            .embed(&[s("alpha"), s("beta"), s("alpha ")])
            .unwrap();
        assert_ne!(v[0], v[1]);
        assert_ne!(v[0], v[2]);
        assert!(
            vector::cosine(&v[0], &v[1]).abs() < 0.3,
            "fake vectors are near-orthogonal"
        );
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert!(FakeEmbedder.embed(&[]).unwrap().is_empty());
        let real = FastEmbedder::new("/nonexistent");
        assert!(real.embed(&[]).unwrap().is_empty());
        assert!(!real.is_loaded(), "empty input must not load the model");
    }

    #[test]
    fn fastembedder_new_does_not_load() {
        let e = FastEmbedder::new("/tmp/does-not-matter");
        assert!(!e.is_loaded());
        assert!(format!("{e:?}").contains("loaded: false"));
    }

    #[test]
    fn embed_real_bge_small_is_384d_unit_length() {
        if std::env::var("EMBED_REAL").as_deref() != Ok("1") {
            eprintln!(
                "skipped: set EMBED_REAL=1 to load the real BGE-small model (downloads ~130 MB)"
            );
            return;
        }
        let cache =
            std::env::var("DAILYBRIEF_MODELS_DIR").unwrap_or_else(|_| "data/models".to_string());
        let e = FastEmbedder::new(cache);
        let v = e
            .embed(&[s("Ownership and borrowing in Rust"), s("Bánh mì Sài Gòn")])
            .unwrap();
        assert!(e.is_loaded());
        assert_eq!(v.len(), 2);
        for row in &v {
            assert_eq!(row.len(), DIMENSIONS);
            let norm: f32 = row.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-4, "norm {norm}");
        }
        assert!(
            vector::cosine(&v[0], &v[1]) < 0.9,
            "unrelated texts are not near-identical"
        );
    }
}
