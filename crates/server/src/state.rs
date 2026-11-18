//! Application state：所有 handler 共用的資源。

use std::sync::Arc;

use hammer_core::attachments::AttachmentStore;
use hammer_core::db::sqlx::PgPool;
use hammer_core::documents::DocumentHandlers;

use crate::Config;

/// 所有 handler 共用的狀態。clone 成本低（內部是 `Arc` 與連線池 handle）。
///
/// 之後的 auth（session 驗證服務等）加在這裡。
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    /// 附件目錄與單檔大小上限（由 `config` 產生）。
    pub attachments: Arc<AttachmentStore>,
    /// 各系統單據的過帳、作廢處理函式（§10「單據動作」）。
    pub documents: DocumentHandlers,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        let attachments =
            AttachmentStore::new(config.attachments_dir.clone(), config.attachment_max_bytes);
        Self {
            pool,
            config: Arc::new(config),
            attachments: Arc::new(attachments),
            documents: DocumentHandlers::standard(),
        }
    }
}
