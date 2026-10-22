//! Bounded chunking for logical Canvas stream events that exceed a platform's
//! local datagram ceiling. The transport is Canvas-owned; producers hand it an
//! opaque serialized event and consumers publish only a verified generation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const CONTRACT: &str = "canvas.stream.chunk.v1";
pub const MAX_GENERATION_BYTES: usize = 64 * 1024;
pub const MAX_CHUNKS: usize = 128;
pub const MAX_ACTIVE_GENERATIONS: usize = 8;
pub const DEFAULT_EXPIRY_MS: u64 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamChunk {
    pub generation_id: String,
    pub index: u16,
    pub count: u16,
    pub payload_sha256: String,
    pub expires_at_ms: u64,
    /// Hex keeps the contract dependency-free and safely splittable at any byte.
    pub payload_hex: String,
}

impl StreamChunk {
    pub fn validate(&self) -> Result<(), String> {
        if self.generation_id.is_empty() || self.generation_id.len() > 128 {
            return Err("invalid chunk generation id".into());
        }
        let count = usize::from(self.count);
        if count == 0 || count > MAX_CHUNKS || usize::from(self.index) >= count {
            return Err("invalid chunk index/count".into());
        }
        if self.payload_sha256.len() != 64
            || !self
                .payload_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("invalid chunk payload digest".into());
        }
        if self.expires_at_ms == 0
            || self.payload_hex.is_empty()
            || self.payload_hex.len() % 2 != 0
            || !self
                .payload_hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("invalid chunk payload".into());
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Reassembler {
    generations: BTreeMap<String, Generation>,
}

struct Generation {
    count: u16,
    digest: String,
    expires_at_ms: u64,
    parts: Vec<Option<Vec<u8>>>,
    bytes: usize,
}

impl Reassembler {
    pub fn push(&mut self, chunk: StreamChunk, now_ms: u64) -> Result<Option<Vec<u8>>, String> {
        self.expire(now_ms);
        chunk.validate()?;
        if chunk.expires_at_ms <= now_ms {
            return Err("chunk generation expired".into());
        }
        if !self.generations.contains_key(&chunk.generation_id)
            && self.generations.len() >= MAX_ACTIVE_GENERATIONS
        {
            return Err("too many active chunk generations".into());
        }
        let bytes = decode_hex(&chunk.payload_hex)?;
        let generation = self
            .generations
            .entry(chunk.generation_id.clone())
            .or_insert_with(|| Generation {
                count: chunk.count,
                digest: chunk.payload_sha256.clone(),
                expires_at_ms: chunk.expires_at_ms,
                parts: vec![None; usize::from(chunk.count)],
                bytes: 0,
            });
        if generation.count != chunk.count || generation.digest != chunk.payload_sha256 {
            self.generations.remove(&chunk.generation_id);
            return Err("chunk generation metadata changed".into());
        }
        let slot = &mut generation.parts[usize::from(chunk.index)];
        if let Some(existing) = slot {
            if *existing != bytes {
                self.generations.remove(&chunk.generation_id);
                return Err("chunk index carried conflicting bytes".into());
            }
            return Ok(None);
        }
        generation.bytes = generation.bytes.saturating_add(bytes.len());
        if generation.bytes > MAX_GENERATION_BYTES {
            self.generations.remove(&chunk.generation_id);
            return Err("chunk generation exceeds Canvas stream limit".into());
        }
        *slot = Some(bytes);
        if generation.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let generation = self.generations.remove(&chunk.generation_id).unwrap();
        let payload = generation
            .parts
            .into_iter()
            .flatten()
            .flatten()
            .collect::<Vec<_>>();
        if sha256_hex(&payload) != generation.digest {
            return Err("chunk generation digest mismatch".into());
        }
        Ok(Some(payload))
    }

    pub fn expire(&mut self, now_ms: u64) {
        self.generations
            .retain(|_, generation| generation.expires_at_ms > now_ms);
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    encode_hex(&digest)
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex byte".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(index: u16, count: u16, bytes: &[u8], whole: &[u8]) -> StreamChunk {
        StreamChunk {
            generation_id: "generation-a".into(),
            index,
            count,
            payload_sha256: sha256_hex(whole),
            expires_at_ms: 1_000,
            payload_hex: encode_hex(bytes),
        }
    }

    #[test]
    fn publishes_only_a_complete_verified_generation() {
        let mut reassembler = Reassembler::default();
        assert_eq!(
            reassembler
                .push(chunk(1, 2, b"world", b"hello world"), 1)
                .unwrap(),
            None
        );
        assert_eq!(
            reassembler
                .push(chunk(0, 2, b"hello ", b"hello world"), 1)
                .unwrap(),
            Some(b"hello world".to_vec())
        );
    }

    #[test]
    fn expired_or_corrupt_generations_never_publish() {
        let mut reassembler = Reassembler::default();
        assert!(
            reassembler
                .push(chunk(0, 1, b"wrong", b"right"), 1)
                .is_err()
        );
        assert!(
            reassembler
                .push(chunk(0, 1, b"right", b"right"), 1_000)
                .is_err()
        );
    }
}
