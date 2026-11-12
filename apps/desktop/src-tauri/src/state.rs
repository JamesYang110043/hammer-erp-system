//! Tauri 管理的全域狀態。

use std::path::PathBuf;
use std::sync::{PoisonError, RwLock};

use crate::api_client::ApiClient;
use crate::error::ClientError;
use crate::settings::Settings;

pub struct AppState {
    pub client: ApiClient,
    settings_path: PathBuf,
    settings: RwLock<Settings>,
}

impl AppState {
    pub fn new(client: ApiClient, settings_path: PathBuf) -> Self {
        let settings = Settings::load(&settings_path);
        Self {
            client,
            settings_path,
            settings: RwLock::new(settings),
        }
    }

    pub fn server_url(&self) -> String {
        self.settings
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .server_url
            .clone()
    }

    /// 更新伺服器網址並寫入設定檔（`server_url` 須已正規化）。
    pub fn set_server_url(&self, server_url: String) -> Result<(), ClientError> {
        let mut settings = self
            .settings
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut next = settings.clone();
        next.server_url = server_url;
        next.save(&self.settings_path)
            .map_err(|err| ClientError::Storage {
                message: format!("儲存設定失敗：{err}"),
            })?;
        *settings = next;
        Ok(())
    }
}
