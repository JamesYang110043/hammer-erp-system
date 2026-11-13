//! 登入 token 的存放位置（`docs/architecture.md` §3.3）。
//!
//! 正式環境存在作業系統鑰匙圈（macOS Keychain、Windows 認證管理員），
//! 以伺服器網址區分：同一台電腦連不同伺服器時各自保留登入狀態。
//! token 只留在 Rust 端，不會傳給前端。

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::error::ClientError;

/// 鑰匙圈的 service 名稱（與 `tauri.conf.json` 的 `identifier` 相同）。
pub const KEYRING_SERVICE: &str = "com.hammererp.desktop";

/// 以伺服器網址（已正規化）為 key 存放 token。
pub trait TokenStore: Send + Sync {
    /// 沒有儲存時回傳 `None`。
    fn get(&self, server_url: &str) -> Result<Option<String>, ClientError>;
    fn set(&self, server_url: &str, token: &str) -> Result<(), ClientError>;
    /// 沒有儲存時也視為成功。
    fn delete(&self, server_url: &str) -> Result<(), ClientError>;
}

/// 作業系統鑰匙圈（`keyring` crate）。
#[derive(Debug, Default)]
pub struct KeyringTokenStore;

impl KeyringTokenStore {
    fn entry(server_url: &str) -> Result<keyring::Entry, ClientError> {
        keyring::Entry::new(KEYRING_SERVICE, server_url).map_err(keyring_error)
    }
}

impl TokenStore for KeyringTokenStore {
    fn get(&self, server_url: &str) -> Result<Option<String>, ClientError> {
        match Self::entry(server_url)?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(keyring_error(err)),
        }
    }

    fn set(&self, server_url: &str, token: &str) -> Result<(), ClientError> {
        Self::entry(server_url)?
            .set_password(token)
            .map_err(keyring_error)
    }

    fn delete(&self, server_url: &str) -> Result<(), ClientError> {
        match Self::entry(server_url)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(keyring_error(err)),
        }
    }
}

fn keyring_error(err: keyring::Error) -> ClientError {
    ClientError::storage(format!("無法存取系統鑰匙圈：{err}"))
}

/// 存在記憶體（測試用，程式關閉後消失）。
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    tokens: Mutex<HashMap<String, String>>,
}

impl TokenStore for MemoryTokenStore {
    fn get(&self, server_url: &str) -> Result<Option<String>, ClientError> {
        Ok(self
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(server_url)
            .cloned())
    }

    fn set(&self, server_url: &str, token: &str) -> Result<(), ClientError> {
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(server_url.to_owned(), token.to_owned());
        Ok(())
    }

    fn delete(&self, server_url: &str) -> Result<(), ClientError> {
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(server_url);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_is_keyed_by_server_url() {
        let store = MemoryTokenStore::default();
        assert_eq!(store.get("http://a").unwrap(), None);
        store.set("http://a", "ta").unwrap();
        store.set("http://b", "tb").unwrap();
        assert_eq!(store.get("http://a").unwrap().as_deref(), Some("ta"));
        store.delete("http://a").unwrap();
        store.delete("http://a").unwrap();
        assert_eq!(store.get("http://a").unwrap(), None);
        assert_eq!(store.get("http://b").unwrap().as_deref(), Some("tb"));
    }
}
