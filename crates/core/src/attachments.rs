//! 附件（`file` 欄位，docs/architecture.md §7.3、§8.4 `attachments`）。
//!
//! - 上傳（需要 `record.write`）：[`AttachmentStore::begin_upload`] → [`Upload::write`]（逐段寫入，
//!   計算 SHA-256、檢查大小上限）→ [`Upload::finish`]（寫入資料列與操作紀錄）
//! - 下載（需要 `record.read`）：[`read_attachment`]
//! - 綁定（新增、修改資料時，在 [`crate::records`] 的 transaction 內）：
//!   [`prepare_file_changes`] 檢查並鎖定附件 → 寫入資料 → [`apply_file_changes`] 綁定新附件、
//!   把被換掉的舊附件標記為刪除
//! - 清理：[`cleanup_unbound`] 刪除未綁定超過 [`UNBOUND_RETENTION_HOURS`] 小時的附件（資料列與檔案）
//!
//! # 磁碟
//!
//! 檔案存在附件目錄（[`AttachmentStore::root`]）下，以附件 id 命名（`storage_path`），原始檔名只存在
//! 資料庫，避免路徑穿越。上傳中的檔案寫在 `tmp/` 子目錄，完成後才改名到正式位置；上傳失敗或中斷時
//! 暫存檔刪除（[`cleanup_unbound`] 也會清掉程序中止時留下的舊暫存檔）。
//!
//! # 操作紀錄
//!
//! `entity_key` 為 [`ATTACHMENT_AUDIT_KEY`]、`record_id` 為附件 id：
//!
//! | 時機 | action | 內容 |
//! |---|---|---|
//! | 上傳 | `create` | `after`：檔名、MIME、大小、SHA-256 |
//! | 綁定 | `update` | 綁定的 `entity_key`、`record_id`、`field_key`（之前為 `null`） |
//! | 欄位值被換掉或清空 | `delete` | `before`：原本綁定的位置 |
//! | 清理未綁定的附件 | `delete` | `before`：檔名等資訊；操作者為 `null` |
//!
//! 純邏輯（[`changed_files`]、[`check_file_targets`]、[`format_size`]）有單元測試；存取資料庫與磁碟的
//! 部分由 `crates/server/tests/attachments_api.rs` 的整合測試涵蓋。

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use hammer_db::DbError;
use hammer_db::attachment::{self as db, AttachmentRow, NewAttachment};
use hammer_db::sqlx::{PgPool, PgTransaction};
use hammer_shared::attachment::{
    Attachment, UNBOUND_RETENTION_HOURS, normalize_mime_type, sanitize_file_name,
};
use hammer_shared::schema::{DataType, FieldDefinition};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use tokio::io::AsyncWriteExt;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use crate::record_data::FieldError;

/// 附件的操作紀錄使用的 `entity_key`。
pub const ATTACHMENT_AUDIT_KEY: &str = "attachments";
/// 附件不存在或已刪除時的錯誤訊息。
pub const MSG_ATTACHMENT_NOT_FOUND: &str = "附件不存在或已刪除";
/// 附件已綁定到其他資料或欄位時的錯誤訊息。
pub const MSG_ATTACHMENT_IN_USE: &str = "附件已被其他資料或欄位使用";
/// 下載時找不到附件的錯誤訊息。
pub const MSG_DOWNLOAD_NOT_FOUND: &str = "找不到此附件";
/// 上傳的檔名處理後是空字串時的錯誤訊息。
pub const MSG_FILE_NAME_REQUIRED: &str = "檔名不可為空";
/// 上傳錯誤的 `details.field`。
pub const UPLOAD_FIELD: &str = hammer_shared::attachment::UPLOAD_FIELD;

/// 暫存檔的子目錄。
const TMP_DIR: &str = "tmp";

// ---------------------------------------------------------------------------
// 儲存位置
// ---------------------------------------------------------------------------

/// 附件目錄與單檔大小上限。
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
    max_bytes: u64,
}

impl AttachmentStore {
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self {
            root: root.into(),
            max_bytes,
        }
    }

    /// 附件目錄。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 單檔大小上限（位元組）。
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn tmp_dir(&self) -> PathBuf {
        self.root.join(TMP_DIR)
    }

    /// `storage_path` 對應的檔案路徑。`storage_path` 必須是單一個一般的檔名（伺服器產生的附件 id），
    /// 否則視為資料庫內容不正確。
    fn path_of(&self, storage_path: &str) -> Result<PathBuf, CoreError> {
        let mut components = Path::new(storage_path).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(_)), None) => Ok(self.root.join(storage_path)),
            _ => Err(CoreError::Internal(format!(
                "附件的 storage_path 不合法：{storage_path}"
            ))),
        }
    }

    /// 開始上傳。需要 `record.write`；在讀取檔案內容之前呼叫，沒有權限時不必接收檔案。
    pub async fn begin_upload(&self, ctx: &AuthContext) -> Result<Upload, CoreError> {
        ctx.require(permission::RECORD_WRITE)?;
        let dir = self.tmp_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| io_error("建立附件暫存目錄", &dir, e))?;
        let path = dir.join(new_id().hyphenated().to_string());
        let file = tokio::fs::File::create_new(&path)
            .await
            .map_err(|e| io_error("建立附件暫存檔", &path, e))?;
        Ok(Upload {
            store: self.clone(),
            file,
            temp: TempFile(Some(path)),
            hasher: Sha256::new(),
            size: 0,
        })
    }
}

/// 上傳中的附件。沒有呼叫 [`Upload::finish`] 就被丟棄時（例如超過大小上限、連線中斷），暫存檔刪除。
pub struct Upload {
    store: AttachmentStore,
    file: tokio::fs::File,
    temp: TempFile,
    hasher: Sha256,
    size: u64,
}

impl std::fmt::Debug for Upload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upload")
            .field("temp", &self.temp.0)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl Upload {
    /// 寫入一段檔案內容。累計超過大小上限 → [`CoreError::PayloadTooLarge`]。
    pub async fn write(&mut self, chunk: &[u8]) -> Result<(), CoreError> {
        let size = self.size.saturating_add(chunk.len() as u64);
        if size > self.store.max_bytes {
            return Err(too_large(self.store.max_bytes));
        }
        self.size = size;
        self.hasher.update(chunk);
        self.file
            .write_all(chunk)
            .await
            .map_err(|e| io_error("寫入附件暫存檔", self.temp.path(), e))
    }

    /// 完成上傳：檔案移到正式位置，寫入一筆未綁定的附件與操作紀錄。
    ///
    /// - `file_name` 處理後（[`sanitize_file_name`]）是空字串 → [`CoreError::InvalidField`]（`file`）
    /// - `mime_type` 沒有或格式不正確時為 `application/octet-stream`（[`normalize_mime_type`]）
    pub async fn finish(
        mut self,
        pool: &PgPool,
        ctx: &AuthContext,
        file_name: Option<&str>,
        mime_type: Option<&str>,
        client_info: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<Attachment, CoreError> {
        let file_name =
            file_name
                .and_then(sanitize_file_name)
                .ok_or_else(|| CoreError::InvalidField {
                    field: UPLOAD_FIELD,
                    message: MSG_FILE_NAME_REQUIRED.to_owned(),
                })?;
        let mime_type = normalize_mime_type(mime_type);
        self.file
            .flush()
            .await
            .map_err(|e| io_error("寫入附件暫存檔", self.temp.path(), e))?;
        self.file
            .sync_all()
            .await
            .map_err(|e| io_error("寫入附件暫存檔", self.temp.path(), e))?;

        let sha256 = hex(&self.hasher.finalize_reset());
        let size_bytes =
            i64::try_from(self.size).map_err(|_| CoreError::Internal("附件大小超出範圍".into()))?;
        let id = new_id();
        let storage_path = id.hyphenated().to_string();
        let target = self.store.path_of(&storage_path)?;

        let mut tx = pool.begin().await.map_err(DbError::from)?;
        db::insert_attachment(
            &mut *tx,
            &NewAttachment {
                id,
                file_name: &file_name,
                mime_type: &mime_type,
                size_bytes,
                sha256: &sha256,
                storage_path: &storage_path,
                created_by: ctx.user_id,
                created_at: now,
            },
        )
        .await?;
        let after = json!({
            "file_name": file_name,
            "mime_type": mime_type,
            "size_bytes": size_bytes,
            "sha256": sha256,
        });
        record_audit(
            &mut tx,
            Some(ctx.user_id),
            action::CREATE,
            id,
            AuditDiff::between(None, Some(&after))?,
            client_info,
            now,
        )
        .await?;

        // 先把檔案移到正式位置再 commit：資料列存在時檔案一定存在
        tokio::fs::rename(self.temp.path(), &target)
            .await
            .map_err(|e| io_error("移動附件檔案", &target, e))?;
        self.temp.disarm();
        if let Err(err) = tx.commit().await {
            remove_file_quietly(&target).await;
            return Err(DbError::from(err).into());
        }

        tracing::info!(actor = %ctx.user_id, attachment_id = %id, size_bytes, "已上傳附件");
        Ok(Attachment {
            id,
            file_name,
            mime_type,
            size_bytes: self.size,
            sha256,
            created_at: now,
        })
    }
}

/// 暫存檔：被丟棄時刪除，除非已 [`TempFile::disarm`]。
struct TempFile(Option<PathBuf>);

impl TempFile {
    fn path(&self) -> &Path {
        self.0.as_deref().unwrap_or(Path::new(""))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take()
            && let Err(err) = std::fs::remove_file(&path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), error = %err, "無法刪除附件暫存檔");
        }
    }
}

// ---------------------------------------------------------------------------
// 下載
// ---------------------------------------------------------------------------

/// 下載的附件：資訊與檔案內容。
#[derive(Debug, Clone)]
pub struct AttachmentFile {
    pub attachment: Attachment,
    pub content: Vec<u8>,
}

/// 讀取附件（含未綁定的、所屬資料已刪除的）。需要 `record.read`。
///
/// - 附件不存在或已標記刪除 → [`CoreError::NotFound`]
/// - 資料列存在但檔案不存在 → [`CoreError::Internal`]
pub async fn read_attachment(
    pool: &PgPool,
    store: &AttachmentStore,
    ctx: &AuthContext,
    id: Id,
) -> Result<AttachmentFile, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let row = db::find_attachment(pool, id)
        .await?
        .ok_or_else(|| CoreError::NotFound(MSG_DOWNLOAD_NOT_FOUND.into()))?;
    let path = store.path_of(&row.storage_path)?;
    let content = tokio::fs::read(&path)
        .await
        .map_err(|e| io_error("讀取附件檔案", &path, e))?;
    Ok(AttachmentFile {
        attachment: attachment_from_row(row)?,
        content,
    })
}

fn attachment_from_row(row: AttachmentRow) -> Result<Attachment, CoreError> {
    Ok(Attachment {
        id: row.id,
        file_name: row.file_name,
        mime_type: row.mime_type,
        size_bytes: u64::try_from(row.size_bytes)
            .map_err(|_| CoreError::Internal("attachments.size_bytes 為負數".into()))?,
        sha256: row.sha256,
        created_at: row.created_at,
    })
}

// ---------------------------------------------------------------------------
// 綁定
// ---------------------------------------------------------------------------

/// `file` 欄位的一個值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileValue {
    pub field_key: String,
    /// 附件 id。
    pub id: Id,
}

/// 寫入一筆資料時 `file` 欄位的變動。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileChanges {
    /// 要綁定到這筆資料的新附件。
    pub bind: Vec<FileValue>,
    /// 被換掉或清空的舊附件，標記為刪除。
    pub release: Vec<FileValue>,
}

impl FileChanges {
    pub fn is_empty(&self) -> bool {
        self.bind.is_empty() && self.release.is_empty()
    }
}

/// `data`（正規化後的完整資料）與 `existing`（修改前的資料；新增時為 `None`）中值不同的 `file` 欄位，
/// 依 `fields` 的順序。新的值要綁定，舊的值要標記刪除。值不是 UUID 的略過（已經過
/// `normalize_record_data`，新的值一定是 UUID；舊的值格式不正確時不處理）。
pub fn changed_files(
    fields: &[FieldDefinition],
    data: &Map<String, Value>,
    existing: Option<&Map<String, Value>>,
) -> FileChanges {
    let mut changes = FileChanges::default();
    for field in fields.iter().filter(|f| f.data_type == DataType::File) {
        let key = field.field_key.as_str();
        let new = data.get(key);
        let old = existing.and_then(|e| e.get(key));
        if new == old {
            continue;
        }
        let value_of = |v: Option<&Value>| {
            let id = Id::try_parse(v?.as_str()?).ok()?;
            Some(FileValue {
                field_key: key.to_owned(),
                id,
            })
        };
        changes.bind.extend(value_of(new));
        changes.release.extend(value_of(old));
    }
    changes
}

/// 檢查要綁定的附件：`found` 為依 id 查到的未刪除附件。每個附件必須存在、未綁定，且同一筆資料中
/// 只能用在一個欄位。錯誤依 `bind` 的順序。
pub fn check_file_targets(
    bind: &[FileValue],
    found: &HashMap<Id, AttachmentRow>,
) -> Vec<FieldError> {
    let mut used = HashSet::new();
    let mut errors = Vec::new();
    for v in bind {
        let message = match found.get(&v.id) {
            None => Some(MSG_ATTACHMENT_NOT_FOUND),
            Some(row) if row.record_id.is_some() => Some(MSG_ATTACHMENT_IN_USE),
            Some(_) if !used.insert(v.id) => Some(MSG_ATTACHMENT_IN_USE),
            Some(_) => None,
        };
        if let Some(message) = message {
            errors.push(FieldError {
                field_key: v.field_key.clone(),
                message: message.to_owned(),
            });
        }
    }
    errors
}

/// 寫入資料前呼叫：找出有變動的 `file` 欄位（[`changed_files`]），鎖定要綁定的附件（`FOR UPDATE`）
/// 並檢查（[`check_file_targets`]）。不符合 → [`CoreError::InvalidFields`]。
///
/// 鎖定到 transaction 結束，避免同一個附件同時被綁到兩筆資料，或在綁定前被清理。
pub async fn prepare_file_changes(
    tx: &mut PgTransaction<'_>,
    fields: &[FieldDefinition],
    data: &Map<String, Value>,
    existing: Option<&Map<String, Value>>,
) -> Result<FileChanges, CoreError> {
    let changes = changed_files(fields, data, existing);
    if changes.bind.is_empty() {
        return Ok(changes);
    }
    let mut ids: Vec<Id> = changes.bind.iter().map(|v| v.id).collect();
    ids.sort();
    ids.dedup();
    let found: HashMap<Id, AttachmentRow> = db::lock_attachments(&mut **tx, &ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let errors = check_file_targets(&changes.bind, &found);
    if errors.is_empty() {
        Ok(changes)
    } else {
        Err(CoreError::InvalidFields(errors))
    }
}

/// 寫入資料後呼叫（同一個 transaction）：把 [`prepare_file_changes`] 檢查過的附件綁定到
/// `(entity_key, record_id)`，被換掉的舊附件標記為刪除（檔案保留），並寫入操作紀錄。
pub async fn apply_file_changes(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    entity_key: &str,
    record_id: Id,
    changes: &FileChanges,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    for v in &changes.bind {
        let bound = db::bind_attachment(
            &mut **tx,
            v.id,
            entity_key,
            record_id,
            &v.field_key,
            ctx.user_id,
        )
        .await?;
        if !bound {
            // prepare_file_changes 已鎖定並確認未綁定
            return Err(CoreError::Internal(format!("附件 {} 無法綁定", v.id)));
        }
        let before = json!({ "entity_key": null, "record_id": null, "field_key": null });
        let after = json!({
            "entity_key": entity_key,
            "record_id": record_id,
            "field_key": v.field_key,
        });
        record_audit(
            tx,
            Some(ctx.user_id),
            action::UPDATE,
            v.id,
            AuditDiff::between(Some(&before), Some(&after))?,
            client_info,
            now,
        )
        .await?;
    }

    if changes.release.is_empty() {
        return Ok(());
    }
    let ids: Vec<Id> = changes.release.iter().map(|v| v.id).collect();
    let released: HashSet<Id> =
        db::mark_attachments_deleted(&mut **tx, entity_key, record_id, &ids, now, ctx.user_id)
            .await?
            .into_iter()
            .collect();
    for v in changes.release.iter().filter(|v| released.contains(&v.id)) {
        let before = json!({
            "entity_key": entity_key,
            "record_id": record_id,
            "field_key": v.field_key,
        });
        record_audit(
            tx,
            Some(ctx.user_id),
            action::DELETE,
            v.id,
            AuditDiff::between(Some(&before), None)?,
            client_info,
            now,
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 清理
// ---------------------------------------------------------------------------

/// [`cleanup_unbound`] 的結果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// 刪除的附件（資料列）數。
    pub attachments: usize,
    /// 刪除的舊暫存檔數。
    pub temp_files: usize,
}

/// 刪除 `now` 的 [`UNBOUND_RETENTION_HOURS`] 小時之前上傳、仍未綁定的附件（資料列與檔案），
/// 以及同樣時限之前留下的暫存檔。已綁定的附件（含所屬資料已刪除、已標記刪除的）不受影響。
///
/// 系統排程執行（伺服器啟動時與每日一次），不檢查權限；操作紀錄的操作者為 `null`。
pub async fn cleanup_unbound(
    pool: &PgPool,
    store: &AttachmentStore,
    now: OffsetDateTime,
) -> Result<CleanupReport, CoreError> {
    let cutoff = now - Duration::hours(UNBOUND_RETENTION_HOURS);

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let rows = db::delete_unbound_before(&mut *tx, cutoff).await?;
    for row in &rows {
        let before = json!({
            "file_name": row.file_name,
            "mime_type": row.mime_type,
            "size_bytes": row.size_bytes,
            "sha256": row.sha256,
        });
        record_audit(
            &mut tx,
            None,
            action::DELETE,
            row.id,
            AuditDiff::between(Some(&before), None)?,
            None,
            now,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;

    // 資料列已刪除後才刪檔案：刪檔失敗只會留下沒有資料列的檔案，不會有資料列指向不存在的檔案
    for row in &rows {
        match store.path_of(&row.storage_path) {
            Ok(path) => remove_file_quietly(&path).await,
            Err(err) => tracing::warn!(error = %err, "略過刪除附件檔案"),
        }
    }
    let temp_files = remove_old_temp_files(&store.tmp_dir(), cutoff.into()).await;

    let report = CleanupReport {
        attachments: rows.len(),
        temp_files,
    };
    if report != CleanupReport::default() {
        tracing::info!(
            attachments = report.attachments,
            temp_files = report.temp_files,
            "已清理未綁定的附件"
        );
    }
    Ok(report)
}

/// 刪除 `dir` 中最後修改時間早於 `cutoff` 的檔案，回傳刪除的數量。目錄不存在時為 0。
async fn remove_old_temp_files(dir: &Path, cutoff: SystemTime) -> usize {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return 0;
    };
    let mut removed = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        let old = meta.modified().is_ok_and(|m| m < cutoff);
        if meta.is_file() && old && tokio::fs::remove_file(entry.path()).await.is_ok() {
            removed += 1;
        }
    }
    removed
}

// ---------------------------------------------------------------------------
// 共用
// ---------------------------------------------------------------------------

/// 大小的顯示文字：整除 1 MiB 時為 `N MB`，否則為位元組數。
pub fn format_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MB", bytes / MIB)
    } else {
        format!("{bytes} 位元組")
    }
}

fn too_large(max_bytes: u64) -> CoreError {
    CoreError::PayloadTooLarge(format!("檔案超過大小上限 {}", format_size(max_bytes)))
}

fn io_error(what: &str, path: &Path, err: std::io::Error) -> CoreError {
    CoreError::Internal(format!("{what}失敗（{}）：{err}", path.display()))
}

async fn remove_file_quietly(path: &Path) {
    if let Err(err) = tokio::fs::remove_file(path).await
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %err, "無法刪除附件檔案");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn record_audit(
    tx: &mut PgTransaction<'_>,
    user_id: Option<Id>,
    action: &str,
    attachment_id: Id,
    diff: AuditDiff,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    audit::record(
        tx,
        &AuditEntry {
            user_id,
            action,
            entity_key: Some(ATTACHMENT_AUDIT_KEY),
            record_id: Some(attachment_id),
            diff,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::schema::FieldStorage;
    use time::macros::datetime;

    fn field(key: &str, data_type: DataType) -> FieldDefinition {
        let now = datetime!(2026-11-14 09:00 +8);
        FieldDefinition {
            id: new_id(),
            entity_id: Id::nil(),
            field_key: key.to_owned(),
            label: key.to_owned(),
            data_type,
            is_system: false,
            storage: FieldStorage::Json,
            is_required: false,
            is_visible: true,
            is_readonly: false,
            is_searchable: false,
            is_unique: false,
            default_value: None,
            validation: None,
            options: None,
            decimal_places: None,
            relation_entity_id: None,
            formula: None,
            formula_result_type: None,
            ui_widget: None,
            column_width: None,
            help_text: None,
            sort_order: 0,
            is_active: true,
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    fn fv(key: &str, n: u128) -> FileValue {
        FileValue {
            field_key: key.to_owned(),
            id: id(n),
        }
    }

    fn row(n: u128, bound: bool) -> AttachmentRow {
        AttachmentRow {
            id: id(n),
            entity_key: bound.then(|| "repair".to_owned()),
            record_id: bound.then(new_id),
            field_key: bound.then(|| "photo".to_owned()),
            file_name: "a.txt".into(),
            mime_type: "text/plain".into(),
            size_bytes: 1,
            sha256: String::new(),
            storage_path: id(n).to_string(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            created_by: None,
            deleted: false,
        }
    }

    #[test]
    fn changed_files_on_create_and_update() {
        let fields = [
            field("photo", DataType::File),
            field("doc", DataType::File),
            field("name", DataType::Text),
        ];
        let a = id(1).to_string();
        let b = id(2).to_string();

        // 新增：有值的 file 欄位都要綁定
        let data = obj(json!({ "photo": a, "name": "x" }));
        assert_eq!(
            changed_files(&fields, &data, None),
            FileChanges {
                bind: vec![fv("photo", 1)],
                release: vec![],
            }
        );

        // 修改：沒變的不處理；換掉的綁新的、釋放舊的；清空的只釋放
        let existing = obj(json!({ "photo": a, "doc": b, "name": "x" }));
        let c = id(3).to_string();
        let data = obj(json!({ "photo": a, "name": "y" }));
        assert_eq!(
            changed_files(&fields, &data, Some(&existing)),
            FileChanges {
                bind: vec![],
                release: vec![fv("doc", 2)],
            }
        );
        let data = obj(json!({ "photo": c, "doc": b }));
        assert_eq!(
            changed_files(&fields, &data, Some(&existing)),
            FileChanges {
                bind: vec![fv("photo", 3)],
                release: vec![fv("photo", 1)],
            }
        );
        assert!(changed_files(&fields, &existing, Some(&existing)).is_empty());

        // 舊值格式不正確時不處理
        let corrupted = obj(json!({ "photo": "not-a-uuid" }));
        assert_eq!(
            changed_files(&fields, &obj(json!({})), Some(&corrupted)),
            FileChanges::default()
        );
    }

    #[test]
    fn check_targets() {
        let found: HashMap<Id, AttachmentRow> = [row(1, false), row(2, true), row(3, false)]
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        assert!(check_file_targets(&[fv("photo", 1), fv("doc", 3)], &found).is_empty());

        let errors = check_file_targets(&[fv("a", 1), fv("b", 2), fv("c", 9), fv("d", 1)], &found);
        let got: Vec<_> = errors
            .iter()
            .map(|e| (e.field_key.as_str(), e.message.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("b", MSG_ATTACHMENT_IN_USE),
                ("c", MSG_ATTACHMENT_NOT_FOUND),
                // 同一個附件不可用在兩個欄位
                ("d", MSG_ATTACHMENT_IN_USE),
            ]
        );
    }

    #[test]
    fn size_text() {
        assert_eq!(format_size(20 * 1024 * 1024), "20 MB");
        assert_eq!(format_size(1024), "1024 位元組");
        assert_eq!(format_size(0), "0 位元組");
    }

    #[test]
    fn storage_path_must_be_single_component() {
        let store = AttachmentStore::new("/srv/files", 10);
        assert_eq!(
            store
                .path_of("0190c3a2-0000-7000-8000-000000000000")
                .unwrap(),
            PathBuf::from("/srv/files/0190c3a2-0000-7000-8000-000000000000")
        );
        for bad in ["", "../x", "a/b", "/etc/passwd", ".."] {
            assert!(store.path_of(bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn upload_over_limit_removes_temp_file() {
        let root = std::env::temp_dir().join(format!("hammer-att-{}", new_id()));
        let store = AttachmentStore::new(&root, 4);
        let mut upload = Upload {
            store: store.clone(),
            file: tokio::fs::File::create_new({
                tokio::fs::create_dir_all(store.tmp_dir()).await.unwrap();
                store.tmp_dir().join("t")
            })
            .await
            .unwrap(),
            temp: TempFile(Some(store.tmp_dir().join("t"))),
            hasher: Sha256::new(),
            size: 0,
        };
        upload.write(b"abc").await.unwrap();
        let err = upload.write(b"de").await.unwrap_err();
        assert!(matches!(err, CoreError::PayloadTooLarge(_)));
        drop(upload);
        assert!(!store.tmp_dir().join("t").exists());
        tokio::fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn old_temp_files_removed() {
        let dir = std::env::temp_dir().join(format!("hammer-att-{}", new_id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("a"), b"x").await.unwrap();
        // 時限在未來：檔案算是舊的
        let future = SystemTime::now() + std::time::Duration::from_secs(60);
        assert_eq!(remove_old_temp_files(&dir, future).await, 1);
        tokio::fs::write(dir.join("b"), b"x").await.unwrap();
        assert_eq!(remove_old_temp_files(&dir, SystemTime::UNIX_EPOCH).await, 0);
        assert_eq!(remove_old_temp_files(&dir.join("missing"), future).await, 0);
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }
}
