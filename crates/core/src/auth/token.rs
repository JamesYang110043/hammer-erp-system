//! Session token（docs/architecture.md §6.2）。
//!
//! - token：作業系統的 CSPRNG 產生 32 bytes，以 base64url（不補 `=`）編碼，共 43 個字元。
//!   只在登入成功時回傳給客戶端一次。
//! - 資料庫只保存 token 的 SHA-256（小寫 hex，64 個字元），見 [`hash_token`]。

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::CoreError;

/// token 的原始長度（bytes）。
pub const TOKEN_BYTES: usize = 32;

/// Session token。`Debug` 不會印出內容，避免寫進 log。
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl SessionToken {
    /// 用 CSPRNG 產生新的 token。
    pub fn generate() -> Result<Self, CoreError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|e| CoreError::Internal(format!("無法產生隨機 token：{e}")))?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// token 字串，回傳給客戶端用。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 取出 token 字串。
    pub fn into_string(self) -> String {
        self.0
    }

    /// 存入資料庫的雜湊值。
    pub fn hash(&self) -> String {
        hash_token(&self.0)
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionToken(***)")
    }
}

/// 計算 token 的 SHA-256，回傳小寫 hex。同一個 token 的結果固定。
///
/// 用於查詢 `sessions.token_hash`，客戶端送來的 token 不需要先檢查格式。
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    hex
}

const HEX: &[u8; 16] = b"0123456789abcdef";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic() {
        let token = SessionToken::generate().unwrap();
        assert_eq!(token.hash(), token.hash());
        assert_eq!(hash_token(token.as_str()), token.hash());
    }

    #[test]
    fn hash_differs_from_token() {
        let token = SessionToken::generate().unwrap();
        assert_ne!(token.hash(), token.as_str());
        assert!(!token.hash().contains(token.as_str()));
    }

    #[test]
    fn hash_matches_known_sha256() {
        // echo -n "abc" | shasum -a 256
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hash_is_64_lowercase_hex() {
        let h = hash_token("anything");
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }

    #[test]
    fn token_is_43_char_base64url_of_32_bytes() {
        let token = SessionToken::generate().unwrap();
        assert_eq!(token.as_str().len(), 43);
        assert!(
            token
                .as_str()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_eq!(
            URL_SAFE_NO_PAD.decode(token.as_str()).unwrap().len(),
            TOKEN_BYTES
        );
    }

    #[test]
    fn tokens_are_unique() {
        let a = SessionToken::generate().unwrap();
        let b = SessionToken::generate().unwrap();
        assert_ne!(a, b);
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn debug_does_not_leak_token() {
        let token = SessionToken::generate().unwrap();
        let debug = format!("{token:?}");
        assert!(!debug.contains(token.as_str()));
    }

    #[test]
    fn into_string_returns_token() {
        let token = SessionToken::generate().unwrap();
        let s = token.as_str().to_owned();
        assert_eq!(token.into_string(), s);
    }
}
