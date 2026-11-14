//! `attachments` 的存取（docs/architecture.md §7.3、§8.4 `attachments`）。
//!
//! - 上傳：[`insert_attachment`]（未綁定）
//! - 下載：[`find_attachment`]（未刪除）
//! - 綁定到資料（`file` 欄位）：[`lock_attachments`]、[`bind_attachment`]
//! - 欄位值被換掉或清空：[`mark_attachments_deleted`]（檔案保留）
//! - 清理：[`delete_unbound_before`]
//!
//! 未綁定的附件 `entity_key`、`record_id`、`field_key` 都是 `NULL`（`ck_attachments_binding`）。

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// `attachments` 的資料列（不含 `updated_*`、`version`）。
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub entity_key: Option<String>,
    pub record_id: Option<Uuid>,
    pub field_key: Option<String>,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub storage_path: String,
    pub created_at: OffsetDateTime,
    pub created_by: Option<Uuid>,
    /// 已標記刪除。
    pub deleted: bool,
}

/// 新增附件所需的資料。
#[derive(Debug, Clone)]
pub struct NewAttachment<'a> {
    pub id: Uuid,
    pub file_name: &'a str,
    pub mime_type: &'a str,
    pub size_bytes: i64,
    pub sha256: &'a str,
    pub storage_path: &'a str,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

/// 新增一筆未綁定的附件。
pub async fn insert_attachment<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewAttachment<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO attachments
            (id, file_name, mime_type, size_bytes, sha256, storage_path,
             created_at, created_by, updated_at, updated_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $7, $8)
        "#,
        new.id,
        new.file_name,
        new.mime_type,
        new.size_bytes,
        new.sha256,
        new.storage_path,
        new.created_at,
        new.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 依 id 查詢未刪除的附件（含未綁定的）。
pub async fn find_attachment<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<AttachmentRow>, DbError> {
    let row = sqlx::query_as!(
        AttachmentRow,
        r#"
        SELECT id, entity_key, record_id, field_key, file_name, mime_type, size_bytes, sha256,
               storage_path, created_at, created_by, deleted_at IS NOT NULL AS "deleted!"
        FROM attachments
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 查詢 `ids` 中未刪除的附件並鎖定（`FOR UPDATE`），依 id 排序。必須在 transaction 內呼叫。
///
/// 鎖定後，清理函式（[`delete_unbound_before`]）與其他寫入會等待到 transaction 結束。
pub async fn lock_attachments<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<AttachmentRow>, DbError> {
    let rows = sqlx::query_as!(
        AttachmentRow,
        r#"
        SELECT id, entity_key, record_id, field_key, file_name, mime_type, size_bytes, sha256,
               storage_path, created_at, created_by, deleted_at IS NOT NULL AS "deleted!"
        FROM attachments
        WHERE id = ANY($1) AND deleted_at IS NULL
        ORDER BY id
        FOR UPDATE
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 把未綁定的附件綁定到資料的欄位。附件已綁定或已刪除時影響 0 筆，回傳 `false`。
pub async fn bind_attachment<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    entity_key: &str,
    record_id: Uuid,
    field_key: &str,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE attachments
        SET entity_key = $2, record_id = $3, field_key = $4, updated_by = $5,
            version = version + 1
        WHERE id = $1 AND record_id IS NULL AND deleted_at IS NULL
        "#,
        id,
        entity_key,
        record_id,
        field_key,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 把綁定在 `(entity_key, record_id)` 上、id 在 `ids` 中的附件標記為刪除（檔案保留），
/// 回傳實際標記的 id。
pub async fn mark_attachments_deleted<'e>(
    executor: impl PgExecutor<'e>,
    entity_key: &str,
    record_id: Uuid,
    ids: &[Uuid],
    deleted_at: OffsetDateTime,
    updated_by: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let rows = sqlx::query_scalar!(
        r#"
        UPDATE attachments
        SET deleted_at = $4, updated_by = $5, version = version + 1
        WHERE id = ANY($3) AND entity_key = $1 AND record_id = $2 AND deleted_at IS NULL
        RETURNING id
        "#,
        entity_key,
        record_id,
        ids,
        deleted_at,
        updated_by,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 刪除 `cutoff` 之前上傳、仍未綁定的附件資料列（含已標記刪除的），回傳被刪除的資料列。
///
/// 正在綁定的附件已被 [`lock_attachments`] 鎖定：這裡會等待該 transaction 結束，
/// 並重新檢查條件，已綁定的不會被刪除。
pub async fn delete_unbound_before<'e>(
    executor: impl PgExecutor<'e>,
    cutoff: OffsetDateTime,
) -> Result<Vec<AttachmentRow>, DbError> {
    let rows = sqlx::query_as!(
        AttachmentRow,
        r#"
        DELETE FROM attachments
        WHERE record_id IS NULL AND created_at < $1
        RETURNING id, entity_key, record_id, field_key, file_name, mime_type, size_bytes, sha256,
                  storage_path, created_at, created_by, deleted_at IS NOT NULL AS "deleted!"
        "#,
        cutoff,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}
