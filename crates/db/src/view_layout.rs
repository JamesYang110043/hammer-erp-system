//! `view_layouts` 的存取（docs/architecture.md §8.5、§9.4）。
//!
//! - 查詢：[`list_layouts`]、[`find_layout`]、[`find_default`]
//! - 寫入：[`find_layout_for_update`]、[`insert_layout`]、[`update_layout`]、[`soft_delete_layout`]
//! - 預設版面：[`list_other_defaults_for_update`]、[`clear_default`]
//!
//! 已軟刪除（`deleted_at IS NOT NULL`）的版面一律視為不存在。
//! 「範圍」指同一個資料類別的同一個使用者（`user_id`），或全域（`user_id IS NULL`）；
//! 每個範圍最多一個預設版面，由唯一索引 [`USER_DEFAULT_UNIQUE`]、[`GLOBAL_DEFAULT_UNIQUE`] 保證。

use serde_json::Value;
use sqlx::PgExecutor;
use sqlx::postgres::PgQueryResult;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// 個人預設版面的唯一索引名稱。
pub const USER_DEFAULT_UNIQUE: &str = "uq_view_layouts_user_default";
/// 全域預設版面的唯一索引名稱。
pub const GLOBAL_DEFAULT_UNIQUE: &str = "uq_view_layouts_global_default";

/// `view_layouts` 的資料列（不含 `deleted_at`）。
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct LayoutRow {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub user_id: Option<Uuid>,
    pub name: String,
    pub config: Value,
    pub is_default: bool,
    pub created_at: OffsetDateTime,
    pub created_by: Option<Uuid>,
    pub updated_at: OffsetDateTime,
    pub updated_by: Option<Uuid>,
    pub version: i32,
}

/// 資料類別的全域版面與 `user_id` 自己的版面：全域在前，再依名稱、id 排序。
pub async fn list_layouts<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<LayoutRow>, DbError> {
    let rows = sqlx::query_as!(
        LayoutRow,
        r#"
        SELECT id, entity_id, user_id, name, config, is_default,
               created_at, created_by, updated_at, updated_by, version
        FROM view_layouts
        WHERE entity_id = $1 AND (user_id IS NULL OR user_id = $2) AND deleted_at IS NULL
        ORDER BY user_id IS NOT NULL, name, id
        "#,
        entity_id,
        user_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 依資料類別與 id 查詢未刪除的版面（不限擁有者，由呼叫端檢查）。
pub async fn find_layout<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    id: Uuid,
) -> Result<Option<LayoutRow>, DbError> {
    let row = sqlx::query_as!(
        LayoutRow,
        r#"
        SELECT id, entity_id, user_id, name, config, is_default,
               created_at, created_by, updated_at, updated_by, version
        FROM view_layouts
        WHERE id = $1 AND entity_id = $2 AND deleted_at IS NULL
        "#,
        id,
        entity_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_layout`]，並鎖定該列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn find_layout_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    id: Uuid,
) -> Result<Option<LayoutRow>, DbError> {
    let row = sqlx::query_as!(
        LayoutRow,
        r#"
        SELECT id, entity_id, user_id, name, config, is_default,
               created_at, created_by, updated_at, updated_by, version
        FROM view_layouts
        WHERE id = $1 AND entity_id = $2 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
        entity_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 範圍內的預設版面：`user_id` 為 `None` 時查詢全域預設。
pub async fn find_default<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Option<LayoutRow>, DbError> {
    let row = sqlx::query_as!(
        LayoutRow,
        r#"
        SELECT id, entity_id, user_id, name, config, is_default,
               created_at, created_by, updated_at, updated_by, version
        FROM view_layouts
        WHERE entity_id = $1 AND user_id IS NOT DISTINCT FROM $2
          AND is_default AND deleted_at IS NULL
        "#,
        entity_id,
        user_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 範圍內 `id` 以外的預設版面，並鎖定（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn list_other_defaults_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    user_id: Option<Uuid>,
    exclude_id: Uuid,
) -> Result<Vec<LayoutRow>, DbError> {
    let rows = sqlx::query_as!(
        LayoutRow,
        r#"
        SELECT id, entity_id, user_id, name, config, is_default,
               created_at, created_by, updated_at, updated_by, version
        FROM view_layouts
        WHERE entity_id = $1 AND user_id IS NOT DISTINCT FROM $2
          AND is_default AND deleted_at IS NULL AND id <> $3
        ORDER BY id
        FOR UPDATE
        "#,
        entity_id,
        user_id,
        exclude_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 取消預設並遞增 `version`。
pub async fn clear_default<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    updated_by: Uuid,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        UPDATE view_layouts
        SET is_default = false, updated_by = $2, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 新增版面所需的資料。
#[derive(Debug, Clone)]
pub struct NewLayout<'a> {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub user_id: Option<Uuid>,
    pub name: &'a str,
    pub config: &'a Value,
    pub is_default: bool,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

/// 新增一個版面。範圍內已有其他預設版面時回傳違反 UNIQUE 限制的 [`DbError::Sqlx`]。
pub async fn insert_layout<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewLayout<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO view_layouts
            (id, entity_id, user_id, name, config, is_default,
             created_at, created_by, updated_at, updated_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $7, $8)
        "#,
        new.id,
        new.entity_id,
        new.user_id,
        new.name,
        new.config,
        new.is_default,
        new.created_at,
        new.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 修改後的版面屬性。
#[derive(Debug, Clone)]
pub struct LayoutUpdate<'a> {
    pub name: &'a str,
    pub config: &'a Value,
    pub is_default: bool,
}

/// 取代版面的屬性並遞增 `version`。版本號不符時影響 0 筆；
/// 範圍內已有其他預設版面時回傳違反 UNIQUE 限制的 [`DbError::Sqlx`]。
pub async fn update_layout<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    update: &LayoutUpdate<'_>,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE view_layouts
        SET name = $3, config = $4, is_default = $5, updated_by = $6, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        update.name,
        update.config,
        update.is_default,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 軟刪除版面並遞增 `version`。版本號不符時影響 0 筆。
pub async fn soft_delete_layout<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    deleted_at: OffsetDateTime,
    deleted_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE view_layouts
        SET deleted_at = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        deleted_at,
        deleted_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}
