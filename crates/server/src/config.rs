//! 伺服器設定。
//!
//! 優先順序：環境變數 > 設定檔 > 預設值。
//!
//! | 環境變數 | 設定檔欄位 | 預設值 |
//! |---|---|---|
//! | `DATABASE_URL` | `database_url` | （必填） |
//! | `HAMMER_LISTEN_ADDR` | `listen_addr` | `127.0.0.1:8080` |
//! | `HAMMER_LOG_LEVEL` | `log_level` | `info` |
//! | `HAMMER_LOG_FORMAT` | `log_format` | 開發版 `pretty`，release 版 `json` |
//! | `HAMMER_ATTACHMENTS_DIR` | `attachments_dir` | `data/attachments` |
//! | `HAMMER_ATTACHMENT_MAX_BYTES` | `attachment_max_bytes` | `20971520`（20 MiB） |
//! | `HAMMER_BODY_LIMIT_BYTES` | `body_limit_bytes` | `10485760`（10 MiB） |
//!
//! 設定檔為 TOML，路徑由 `HAMMER_CONFIG` 指定；沒有設定時不讀取設定檔。
//! 範例見 `crates/server/config.example.toml`。

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

/// 指定設定檔路徑的環境變數。
pub const CONFIG_PATH_ENV: &str = "HAMMER_CONFIG";

/// 設定載入錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("無法讀取設定檔 {path}：{source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("設定檔格式錯誤：{0}")]
    ParseFile(#[from] toml::de::Error),
    #[error("缺少必要設定 {0}")]
    Missing(&'static str),
    #[error("設定 {name} 的值「{value}」無效：{reason}")]
    Invalid {
        name: &'static str,
        value: String,
        reason: String,
    },
}

/// Log 輸出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// 易讀格式，開發用。
    Pretty,
    /// JSON 格式，正式環境用。
    Json,
}

impl LogFormat {
    /// 開發版（debug build）用 `Pretty`，release 版用 `Json`。
    pub fn default_for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Pretty
        } else {
            Self::Json
        }
    }
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pretty" => Ok(Self::Pretty),
            "json" => Ok(Self::Json),
            _ => Err("只能是 pretty 或 json".to_owned()),
        }
    }
}

/// 伺服器設定。
#[derive(Clone)]
pub struct Config {
    /// PostgreSQL 連線字串。
    pub database_url: String,
    /// HTTP 監聽位址。
    pub listen_addr: SocketAddr,
    /// log 等級，`tracing_subscriber::EnvFilter` 的語法（例如 `info`、`hammer_server=debug,info`）。
    pub log_level: String,
    /// log 輸出格式。
    pub log_format: LogFormat,
    /// 附件存放目錄。
    pub attachments_dir: PathBuf,
    /// 附件單檔大小上限（位元組）。上傳附件的路由不受 [`Config::body_limit_bytes`] 限制。
    pub attachment_max_bytes: u64,
    /// Request body 大小上限（位元組）。
    pub body_limit_bytes: usize,
}

// 連線字串含密碼，不可輸出到 log
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"<redacted>")
            .field("listen_addr", &self.listen_addr)
            .field("log_level", &self.log_level)
            .field("log_format", &self.log_format)
            .field("attachments_dir", &self.attachments_dir)
            .field("attachment_max_bytes", &self.attachment_max_bytes)
            .field("body_limit_bytes", &self.body_limit_bytes)
            .finish()
    }
}

/// 設定檔內容；每個欄位都可省略。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    database_url: Option<String>,
    listen_addr: Option<SocketAddr>,
    log_level: Option<String>,
    log_format: Option<LogFormat>,
    attachments_dir: Option<PathBuf>,
    attachment_max_bytes: Option<u64>,
    body_limit_bytes: Option<usize>,
}

impl Config {
    pub const DEFAULT_LISTEN_ADDR: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);
    pub const DEFAULT_LOG_LEVEL: &str = "info";
    pub const DEFAULT_ATTACHMENTS_DIR: &str = "data/attachments";
    pub const DEFAULT_ATTACHMENT_MAX_BYTES: u64 = 20 * 1024 * 1024;
    pub const DEFAULT_BODY_LIMIT_BYTES: usize = 10 * 1024 * 1024;

    /// 從程序的環境變數與 `HAMMER_CONFIG` 指定的設定檔載入。
    pub fn load() -> Result<Self, ConfigError> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let file = match env(CONFIG_PATH_ENV) {
            Some(path) => Some(read_file(Path::new(&path))?),
            None => None,
        };
        Self::from_sources(file.as_deref(), env)
    }

    /// 由設定檔內容（TOML 字串）與環境變數查詢函式組出設定。
    pub fn from_sources(
        file: Option<&str>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let file: FileConfig = match file {
            Some(s) => toml::from_str(s)?,
            None => FileConfig::default(),
        };

        let database_url = env("DATABASE_URL")
            .or(file.database_url)
            .ok_or(ConfigError::Missing("DATABASE_URL"))?;

        let attachment_max_bytes = parse_env(&env, "HAMMER_ATTACHMENT_MAX_BYTES")?
            .or(file.attachment_max_bytes)
            .unwrap_or(Self::DEFAULT_ATTACHMENT_MAX_BYTES);
        if attachment_max_bytes == 0 {
            return Err(ConfigError::Invalid {
                name: "HAMMER_ATTACHMENT_MAX_BYTES",
                value: attachment_max_bytes.to_string(),
                reason: "必須大於 0".to_owned(),
            });
        }

        Ok(Self {
            database_url,
            listen_addr: parse_env(&env, "HAMMER_LISTEN_ADDR")?
                .or(file.listen_addr)
                .unwrap_or(Self::DEFAULT_LISTEN_ADDR),
            log_level: env("HAMMER_LOG_LEVEL")
                .or(file.log_level)
                .unwrap_or_else(|| Self::DEFAULT_LOG_LEVEL.to_owned()),
            log_format: parse_env(&env, "HAMMER_LOG_FORMAT")?
                .or(file.log_format)
                .unwrap_or_else(LogFormat::default_for_build),
            attachments_dir: env("HAMMER_ATTACHMENTS_DIR")
                .map(PathBuf::from)
                .or(file.attachments_dir)
                .unwrap_or_else(|| PathBuf::from(Self::DEFAULT_ATTACHMENTS_DIR)),
            attachment_max_bytes,
            body_limit_bytes: parse_env(&env, "HAMMER_BODY_LIMIT_BYTES")?
                .or(file.body_limit_bytes)
                .unwrap_or(Self::DEFAULT_BODY_LIMIT_BYTES),
        })
    }
}

fn read_file(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_owned(),
        source,
    })
}

fn parse_env<T>(
    env: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<Option<T>, ConfigError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    env(name)
        .map(|value| {
            value
                .trim()
                .parse()
                .map_err(|err: T::Err| ConfigError::Invalid {
                    name,
                    reason: err.to_string(),
                    value,
                })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn defaults_with_only_database_url() {
        let config =
            Config::from_sources(None, env_of(&[("DATABASE_URL", "postgres://x")])).unwrap();
        assert_eq!(config.database_url, "postgres://x");
        assert_eq!(config.listen_addr.to_string(), "127.0.0.1:8080");
        assert_eq!(config.log_level, "info");
        assert_eq!(config.log_format, LogFormat::default_for_build());
        assert_eq!(config.attachments_dir, PathBuf::from("data/attachments"));
        assert_eq!(config.body_limit_bytes, 10 * 1024 * 1024);
        assert_eq!(config.attachment_max_bytes, 20 * 1024 * 1024);
    }

    #[test]
    fn attachment_max_bytes_from_env_and_file() {
        let config = Config::from_sources(
            Some("attachment_max_bytes = 1024"),
            env_of(&[("DATABASE_URL", "postgres://x")]),
        )
        .unwrap();
        assert_eq!(config.attachment_max_bytes, 1024);
        let config = Config::from_sources(
            Some("attachment_max_bytes = 1024"),
            env_of(&[
                ("DATABASE_URL", "postgres://x"),
                ("HAMMER_ATTACHMENT_MAX_BYTES", "2048"),
            ]),
        )
        .unwrap();
        assert_eq!(config.attachment_max_bytes, 2048);
        for bad in ["0", "-1", "abc"] {
            let err = Config::from_sources(
                None,
                env_of(&[
                    ("DATABASE_URL", "postgres://x"),
                    ("HAMMER_ATTACHMENT_MAX_BYTES", bad),
                ]),
            )
            .unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        name: "HAMMER_ATTACHMENT_MAX_BYTES",
                        ..
                    }
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn missing_database_url_is_error() {
        let err = Config::from_sources(None, env_of(&[])).unwrap_err();
        assert!(matches!(err, ConfigError::Missing("DATABASE_URL")));
    }

    #[test]
    fn reads_file() {
        let file = r#"
            database_url = "postgres://file"
            listen_addr = "0.0.0.0:9000"
            log_level = "debug"
            log_format = "json"
            attachments_dir = "/srv/files"
            body_limit_bytes = 1024
        "#;
        let config = Config::from_sources(Some(file), env_of(&[])).unwrap();
        assert_eq!(config.database_url, "postgres://file");
        assert_eq!(config.listen_addr.to_string(), "0.0.0.0:9000");
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.log_format, LogFormat::Json);
        assert_eq!(config.attachments_dir, PathBuf::from("/srv/files"));
        assert_eq!(config.body_limit_bytes, 1024);
    }

    #[test]
    fn env_overrides_file() {
        let file = r#"
            database_url = "postgres://file"
            listen_addr = "0.0.0.0:9000"
            log_format = "json"
        "#;
        let env = env_of(&[
            ("DATABASE_URL", "postgres://env"),
            ("HAMMER_LISTEN_ADDR", "127.0.0.1:7000"),
            ("HAMMER_LOG_FORMAT", "Pretty"),
            ("HAMMER_BODY_LIMIT_BYTES", "2048"),
        ]);
        let config = Config::from_sources(Some(file), env).unwrap();
        assert_eq!(config.database_url, "postgres://env");
        assert_eq!(config.listen_addr.to_string(), "127.0.0.1:7000");
        assert_eq!(config.log_format, LogFormat::Pretty);
        assert_eq!(config.body_limit_bytes, 2048);
    }

    #[test]
    fn invalid_env_value_is_error() {
        let env = env_of(&[
            ("DATABASE_URL", "postgres://x"),
            ("HAMMER_LISTEN_ADDR", "not-an-addr"),
        ]);
        let err = Config::from_sources(None, env).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                name: "HAMMER_LISTEN_ADDR",
                ..
            }
        ));
    }

    #[test]
    fn unknown_file_field_is_error() {
        let err = Config::from_sources(Some("databse_url = \"typo\""), env_of(&[])).unwrap_err();
        assert!(matches!(err, ConfigError::ParseFile(_)));
    }

    #[test]
    fn debug_hides_database_url() {
        let config = Config::from_sources(
            None,
            env_of(&[("DATABASE_URL", "postgres://u:secret@h/db")]),
        )
        .unwrap();
        assert!(!format!("{config:?}").contains("secret"));
    }
}
