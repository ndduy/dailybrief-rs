//! 384-d embedding vectors stored as little-endian `f32` blobs (`SPEC.md` §5).
//! Bytes are decoded with `chunks_exact(4)` + `from_le_bytes`; never a pointer cast (alignment).

pub const DIMENSIONS: usize = 384;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VectorError {
    #[error("vector blob has {0} bytes, not a multiple of 4")]
    BadLength(usize),
    #[error("vector has {got} dimensions, expected {expected}")]
    WrongDimension { expected: usize, got: usize },
}

/// Decodes a stored blob into `DIMENSIONS` floats.
pub fn from_blob(bytes: &[u8]) -> Result<Vec<f32>, VectorError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(VectorError::BadLength(bytes.len()));
    }
    let got = bytes.len() / 4;
    if got != DIMENSIONS {
        return Err(VectorError::WrongDimension {
            expected: DIMENSIONS,
            got,
        });
    }
    let (chunks, _rest) = bytes.as_chunks::<4>();
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// Encodes floats as the little-endian blob the schema expects.
pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Cosine similarity; 0.0 when either vector is empty, of a different length, or all zeros.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Scales `v` to unit length in place; a zero vector is left untouched.
pub fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(i: usize) -> Vec<f32> {
        let mut v = vec![0.0; DIMENSIONS];
        v[i] = 1.0;
        v
    }

    #[test]
    fn roundtrip_le_bytes() {
        let v: Vec<f32> = (0..DIMENSIONS).map(|i| i as f32 * 0.25 - 3.0).collect();
        let blob = to_blob(&v);
        assert_eq!(blob.len(), DIMENSIONS * 4);
        assert_eq!(&blob[..4], &(-3.0f32).to_le_bytes());
        assert_eq!(from_blob(&blob).unwrap(), v);
    }

    #[test]
    fn rejects_odd_length_blob() {
        assert_eq!(
            from_blob(&[1, 2, 3]).unwrap_err(),
            VectorError::BadLength(3)
        );
    }

    #[test]
    fn rejects_wrong_dimension() {
        let blob = to_blob(&[1.0, 2.0]);
        assert_eq!(
            from_blob(&blob).unwrap_err(),
            VectorError::WrongDimension {
                expected: DIMENSIONS,
                got: 2
            }
        );
    }

    #[test]
    fn cosine_of_identical_is_one() {
        let v: Vec<f32> = (0..DIMENSIONS).map(|i| (i % 7) as f32 + 0.5).collect();
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_is_zero() {
        assert_eq!(cosine(&unit(0), &unit(1)), 0.0);
    }

    #[test]
    fn cosine_is_zero_for_mismatch_or_zero_vectors() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
    }

    #[test]
    fn normalize_gives_unit_length_and_leaves_zero_alone() {
        let mut v = vec![3.0, 4.0];
        normalize(&mut v);
        assert_eq!(v, vec![0.6, 0.8]);
        let mut z = vec![0.0, 0.0];
        normalize(&mut z);
        assert_eq!(z, vec![0.0, 0.0]);
    }
}
