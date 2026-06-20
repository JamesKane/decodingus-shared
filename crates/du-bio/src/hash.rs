//! SHA-256 helpers shared across DecodingUs crates: a byte-slice digest, a streaming reader
//! digest, and a file convenience. All return the lowercase-hex digest (the form used for asset
//! manifests, content fingerprints, and pinned-hash verification).

use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Lowercase-hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

/// Lowercase-hex SHA-256 of everything `reader` yields, streamed in 1 MiB chunks (so large
/// inputs are hashed without buffering the whole thing in memory).
pub fn sha256_reader<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Lowercase-hex SHA-256 of a file's contents (streamed — safe for large alignments).
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    sha256_reader(&mut file)
}

/// Format a digest (or any bytes) as lowercase hex.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_has_the_known_sha256() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn reader_matches_byte_slice() {
        let data = b"the quick brown fox jumps over the lazy dog";
        let mut cursor = std::io::Cursor::new(&data[..]);
        assert_eq!(sha256_reader(&mut cursor).unwrap(), sha256_hex(data));
    }
}
