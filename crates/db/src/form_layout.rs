//! `form_layouts` 的存取（docs/architecture.md §7.7、§8.5、§9.4）。
//!
//! 每個資料類別最多一筆未刪除的版面，由唯一索引 [`ENTITY_UNIQUE`] 保證。
//! 已軟刪除（`deleted_at IS NOT NULL`）的版面一律視為不存在。

use serde_json::Value;
use sqlx::PgExecutor;
use sqlx::postgres::PgQueryResult;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// 「每個資料類別最多一筆版面」的唯一索引名稱。
pub const ENTITY_UNIQUE: &str = "uq_form_layouts_entity";

/// `form_layouts` 的資料列（不含 `deleted_at`）。
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct FormLayoutRow {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub config: Value,
    pub version: i32,
}

/// 資料類別未刪除的版面。
pub async fn find_by_entity<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
) -> Result<Option<FormLayoutRow>, DbError> {
    let row = sqlx::query_as!(
        FormLayoutRow,
        r#"
        SELECT id, entity_id, config, version
        FROM form_layouts
        WHERE entity_id = $1 AND deleted_at IS NULL
        "#,
        entity_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_by_entity`]，並鎖定該列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn find_by_entity_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
) -> Result<Option<FormLayoutRow>, DbError> {
    let row = sqlx::query_as!(
        FormLayoutRow,
        r#"
        SELECT id, entity_id, config, version
        FROM form_layouts
        WHERE entity_id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        entity_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 新增版面。同一個資料類別已有版面時回傳違反 [`ENTITY_UNIQUE`] 的 [`DbError::Sqlx`]。
pub async fn insert<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    entity_id: Uuid,
    config: &Value,
    created_by: Uuid,
    created_at: OffsetDateTime,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO form_layouts
            (id, entity_id, config, created_at, created_by, updated_at, updated_by)
        VALUES ($1, $2, $3, $4, $5, $4, $5)
        "#,
        id,
        entity_id,
        config,
        created_at,
        created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 取代 `config` 並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    config: &Value,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE form_layouts
        SET config = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        config,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 軟刪除版面（還原成預設）並遞增 `version`。版本號不符時影響 0 筆。
pub async fn soft_delete<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    deleted_at: OffsetDateTime,
    deleted_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE form_layouts
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
