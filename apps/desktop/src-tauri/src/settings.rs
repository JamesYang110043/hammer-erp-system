//! 本機設定（伺服器網址），存在作業系統的 app 設定目錄下的 `settings.json`。

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::ClientError;

/// 設定檔名稱。
pub const SETTINGS_FILE: &str = "settings.json";

/// 預設伺服器網址（第一階段單機，`docs/architecture.md` §3.1）。
pub const DEFAULT_SERVER_URL: &str = "http://localhost:8080";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 伺服器網址，已正規化（見 [`normalize_server_url`]）。
    pub server_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_owned(),
        }
    }
}

impl Settings {
    /// 讀取設定檔。檔案不存在或內容無法解析時使用預設值，不會失敗。
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut settings: Self = match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(err) => {
                eprintln!("設定檔 {} 格式錯誤，改用預設值：{err}", path.display());
                return Self::default();
            }
        };
        match normalize_server_url(&settings.server_url) {
            Ok(url) => settings.server_url = url,
            Err(_) => settings.server_url = DEFAULT_SERVER_URL.to_owned(),
        }
        settings
    }

    /// 寫入設定檔（先寫暫存檔再改名，避免寫到一半損毀）。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

/// 檢查並正規化伺服器網址：去除前後空白與結尾的 `/`。
///
/// 只接受 `http`、`https`，不可包含帳密、查詢字串或 `#`。
pub fn normalize_server_url(input: &str) -> Result<String, ClientError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ClientError::invalid_input("請輸入伺服器網址"));
    }
    let url = Url::parse(input).map_err(|_| {
        ClientError::invalid_input("伺服器網址格式錯誤，例如：http://localhost:8080")
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ClientError::invalid_input(
            "伺服器網址必須以 http:// 或 https:// 開頭",
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(ClientError::invalid_input("伺服器網址缺少主機名稱"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ClientError::invalid_input("伺服器網址不可包含帳號密碼"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(ClientError::invalid_input(
            "伺服器網址不可包含查詢字串（?）或 #",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_valid_urls() {
        let cases = [
            ("http://localhost:8080", "http://localhost:8080"),
            ("  http://localhost:8080/  ", "http://localhost:8080"),
            ("HTTPS://ERP.example.ts.net", "https://erp.example.ts.net"),
            (
                "http://192.168.1.10:8080/erp/",
                "http://192.168.1.10:8080/erp",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_server_url(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn rejects_invalid_urls() {
        for input in [
            "",
            "   ",
            "localhost:8080",
            "ftp://localhost",
            "http://",
            "http://user:pw@localhost",
            "http://localhost?x=1",
            "http://localhost#a",
        ] {
            assert!(
                matches!(
                    normalize_server_url(input),
                    Err(ClientError::InvalidInput { .. })
                ),
                "{input:?} 應該被拒絕"
            );
        }
    }

    #[test]
    fn load_missing_file_uses_default() {
        let dir = temp_dir("missing");
        assert_eq!(
            Settings::load(&dir.join(SETTINGS_FILE)),
            Settings::default()
        );
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("nested").join(SETTINGS_FILE);
        let settings = Settings {
            server_url: "http://10.0.0.5:8080".to_owned(),
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn load_corrupted_file_uses_default() {
        let dir = temp_dir("corrupted");
        let path = dir.join(SETTINGS_FILE);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());

        std::fs::write(&path, r#"{"server_url":"ftp://x"}"#).unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hammer-desktop-settings-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
