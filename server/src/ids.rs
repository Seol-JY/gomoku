//! Random identifiers: UUIDs, bearer tokens, invite codes, guest names

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use proto::{INVITE_ALPHABET, INVITE_CODE_LEN};
use rand::{Rng, RngExt};
use sha2::{Digest, Sha256};

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// 32 random bytes, base64url, no padding
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Only the hash stored
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

pub fn new_invite_code() -> String {
    let mut rng = rand::rng();
    (0..INVITE_CODE_LEN)
        .map(|_| INVITE_ALPHABET[rng.random_range(0..INVITE_ALPHABET.len())] as char)
        .collect()
}

pub fn new_guest_name() -> String {
    let mut rng = rand::rng();
    let suffix: String = (0..4)
        .map(|_| INVITE_ALPHABET[rng.random_range(0..INVITE_ALPHABET.len())] as char)
        .collect();
    format!("Guest-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_code_shape() {
        for _ in 0..100 {
            assert!(proto::is_valid_invite_code(&new_invite_code()));
        }
    }

    #[test]
    fn token_hash_is_stable_hex() {
        let t = new_token();
        assert_eq!(hash_token(&t), hash_token(&t));
        assert_eq!(hash_token(&t).len(), 64);
        assert_ne!(hash_token(&t), hash_token(&new_token()));
    }
}
