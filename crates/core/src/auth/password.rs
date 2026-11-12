//! 密碼雜湊（Argon2id）與密碼規則（docs/architecture.md §6.2）。
//!
//! 雜湊使用 `argon2` crate 的預設參數（Argon2id v19、m=19 MiB、t=2、p=1，
//! 符合 OWASP 建議），結果為 PHC 字串格式，參數與 salt 都包含在字串中，
//! 之後調整參數也能驗證舊的雜湊。

use argon2::Argon2;
use argon2::password_hash::{self, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

use crate::CoreError;

/// 密碼最少字元數（以 Unicode 字元計算，不是 bytes）。
pub const MIN_PASSWORD_CHARS: usize = 10;

/// 檢查密碼是否符合規則：至少 [`MIN_PASSWORD_CHARS`] 個字元。
///
/// 不符合時回傳 [`CoreError::Validation`]（`VALIDATION`）。
pub fn validate_password_rules(password: &str) -> Result<(), CoreError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(CoreError::Validation(format!(
            "密碼至少需要 {MIN_PASSWORD_CHARS} 個字元"
        )));
    }
    Ok(())
}

/// 以 Argon2id 雜湊密碼。每次使用新的隨機 salt，同一個密碼的結果每次都不同。
///
/// 只負責雜湊，不檢查密碼規則；設定新密碼前先呼叫 [`validate_password_rules`]。
pub fn hash_password(password: &str) -> Result<String, CoreError> {
    let mut salt_bytes = [0u8; password_hash::Salt::RECOMMENDED_LENGTH];
    getrandom::fill(&mut salt_bytes)
        .map_err(|e| CoreError::Internal(format!("無法產生隨機 salt：{e}")))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| CoreError::Internal(format!("salt 編碼失敗：{e}")))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| CoreError::Internal(format!("密碼雜湊失敗：{e}")))?;
    Ok(hash.to_string())
}

/// 驗證密碼與雜湊是否相符。
///
/// 密碼錯誤回傳 `Ok(false)`；雜湊字串格式損壞時回傳 [`CoreError::Internal`]。
pub fn verify_password(password: &str, password_hash: &str) -> Result<bool, CoreError> {
    let parsed = PasswordHash::new(password_hash)
        .map_err(|e| CoreError::Internal(format!("密碼雜湊格式錯誤：{e}")))?;
    match Argon2::default().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(password_hash::Error::Password) => Ok(false),
        Err(e) => Err(CoreError::Internal(format!("密碼驗證失敗：{e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;

    const PASSWORD: &str = "correct horse battery";

    #[test]
    fn correct_password_verifies() {
        let hash = hash_password(PASSWORD).unwrap();
        assert!(verify_password(PASSWORD, &hash).unwrap());
    }

    #[test]
    fn wrong_password_fails() {
        let hash = hash_password(PASSWORD).unwrap();
        assert!(!verify_password("wrong password!", &hash).unwrap());
        assert!(!verify_password("", &hash).unwrap());
    }

    #[test]
    fn same_password_hashes_differently() {
        let a = hash_password(PASSWORD).unwrap();
        let b = hash_password(PASSWORD).unwrap();
        assert_ne!(a, b, "每次雜湊都應使用不同的 salt");
        assert!(verify_password(PASSWORD, &a).unwrap());
        assert!(verify_password(PASSWORD, &b).unwrap());
    }

    #[test]
    fn hash_is_argon2id_and_not_plaintext() {
        let hash = hash_password(PASSWORD).unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(!hash.contains(PASSWORD));
    }

    #[test]
    fn malformed_hash_is_internal_error() {
        let err = verify_password(PASSWORD, "not-a-phc-string").unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal);
    }

    #[test]
    fn password_shorter_than_min_is_rejected() {
        let err = validate_password_rules("123456789").unwrap_err();
        assert_eq!(err.code(), ErrorCode::Validation);
        assert_eq!(err.to_string(), "密碼至少需要 10 個字元");
        assert!(validate_password_rules("").is_err());
    }

    #[test]
    fn password_with_min_length_is_accepted() {
        assert!(validate_password_rules("1234567890").is_ok());
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // 9 個中文字是 27 bytes，仍然不足 10 個字元
        assert!(validate_password_rules("一二三四五六七八九").is_err());
        assert!(validate_password_rules("一二三四五六七八九十").is_ok());
    }
}
