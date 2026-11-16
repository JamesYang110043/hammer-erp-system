//! 客製化模組的資料類別與欄位定義存取（docs/architecture.md §7、§8.5）。
//!
//! - `entity_definitions`：[`list_entities`]、[`find_entity_by_key`]、[`find_entity_by_id`]、
//!   [`find_entity_by_key_for_update`]、[`find_entity_by_id_for_share`]、[`list_detail_entities`]、[`list_detail_entities_for_share`]、
//!   [`list_entities_by_keys_for_update`]、[`list_detail_entities_for_update`]、
//!   [`insert_entity`]、[`update_entity`]、[`update_entity_sort_order`]、[`soft_delete_entity`]、
//!   [`list_relation_references`]
//! - `field_definitions`：[`list_fields`]、[`list_fields_for_update`]、[`find_field`]、
//!   [`find_field_for_update`]、[`insert_field`]、[`update_field`]、[`update_field_sort_order`]、
//!   [`list_formula_deps`]、[`list_formula_field_references`]、[`list_formula_entity_references`]
//! - `schema_change_logs`：[`insert_schema_change_log`]
//!
//! 已軟刪除（`deleted_at IS NOT NULL`）的資料類別與欄位一律視為不存在。
//! 列舉欄位以字串存取，轉換與檢查在 `hammer_core::schema`。
//! 更新語句都帶版本條件（§9.4），結果交給 [`crate::ensure_version_matched`]。

use serde_json::Value;
use sqlx::PgExecutor;
use sqlx::postgres::PgQueryResult;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// `entity_definitions.key` 的 UNIQUE 限制名稱。
pub const ENTITY_KEY_UNIQUE: &str = "entity_definitions_key_key";
/// `field_definitions (entity_id, field_key)` 的 UNIQUE 限制名稱。
pub const FIELD_KEY_UNIQUE: &str = "field_definitions_entity_field_key_key";

// ---------------------------------------------------------------------------
// entity_definitions
// ---------------------------------------------------------------------------

/// `entity_definitions` 的資料列（不含 `deleted_at`、`created_by`、`updated_by`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRow {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    pub icon: Option<String>,
    pub kind: String,
    pub is_system: bool,
    pub storage: String,
    pub table_name: Option<String>,
    pub parent_entity_id: Option<Uuid>,
    pub numbering_pattern: Option<String>,
    pub title_template: Option<String>,
    pub sort_order: i32,
    pub is_active: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub version: i32,
}

/// 列出未刪除的資料類別，依 `sort_order`、`id` 排序。
/// `include_inactive` 為 `false` 時只列出啟用中的資料類別。
pub async fn list_entities<'e>(
    executor: impl PgExecutor<'e>,
    include_inactive: bool,
) -> Result<Vec<EntityRow>, DbError> {
    let rows = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE ($1 OR is_active) AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        include_inactive,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 依代碼查詢未刪除的資料類別（含已停用）。
pub async fn find_entity_by_key<'e>(
    executor: impl PgExecutor<'e>,
    key: &str,
) -> Result<Option<EntityRow>, DbError> {
    let row = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE key = $1 AND deleted_at IS NULL
        "#,
        key,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 依 id 查詢未刪除的資料類別（含已停用）。
pub async fn find_entity_by_id<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<EntityRow>, DbError> {
    let row = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 依代碼查詢未刪除的資料類別並鎖定該列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn find_entity_by_key_for_update<'e>(
    executor: impl PgExecutor<'e>,
    key: &str,
) -> Result<Option<EntityRow>, DbError> {
    let row = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE key = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        key,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_entity_by_key`]，並以共用鎖鎖定該列（`FOR SHARE`）。必須在 transaction 內呼叫。
///
/// 寫入資料時使用：可與其他寫入資料的 transaction 同時進行，但會等待持有 `FOR UPDATE` 的
/// 定義變更（例如變更欄位型態）完成，避免依舊的欄位定義寫入資料。
pub async fn find_entity_by_key_for_share<'e>(
    executor: impl PgExecutor<'e>,
    key: &str,
) -> Result<Option<EntityRow>, DbError> {
    let row = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE key = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        key,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_entity_by_id`]，並以 `FOR SHARE` 鎖定該列。必須在 transaction 內呼叫。
pub async fn find_entity_by_id_for_share<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<EntityRow>, DbError> {
    let row = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE id = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 列出主表 `parent_entity_id` 的明細子表（未刪除，含已停用），依 `sort_order`、`id` 排序，
/// 並以 `FOR SHARE` 鎖定這些列（等待進行中的欄位定義變更完成）。必須在 transaction 內呼叫。
pub async fn list_detail_entities_for_share<'e>(
    executor: impl PgExecutor<'e>,
    parent_entity_id: Uuid,
) -> Result<Vec<EntityRow>, DbError> {
    let rows = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE parent_entity_id = $1 AND kind = 'detail' AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR SHARE
        "#,
        parent_entity_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 列出主表 `parent_entity_id` 的明細子表（未刪除，含已停用），依 `sort_order`、`id` 排序。
pub async fn list_detail_entities<'e>(
    executor: impl PgExecutor<'e>,
    parent_entity_id: Uuid,
) -> Result<Vec<EntityRow>, DbError> {
    let rows = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE parent_entity_id = $1 AND kind = 'detail' AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        parent_entity_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 依代碼列出未刪除的資料類別（含已停用），依 `id` 排序，並鎖定這些列（`FOR UPDATE`）。
/// 找不到的代碼不會出現在結果中。必須在 transaction 內呼叫。
pub async fn list_entities_by_keys_for_update<'e>(
    executor: impl PgExecutor<'e>,
    keys: &[String],
) -> Result<Vec<EntityRow>, DbError> {
    let rows = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE key = ANY($1) AND deleted_at IS NULL
        ORDER BY id
        FOR UPDATE
        "#,
        keys,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 同 [`list_detail_entities`]，並鎖定這些列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn list_detail_entities_for_update<'e>(
    executor: impl PgExecutor<'e>,
    parent_entity_id: Uuid,
) -> Result<Vec<EntityRow>, DbError> {
    let rows = sqlx::query_as!(
        EntityRow,
        r#"
        SELECT id, key, label, icon, kind, is_system, storage, table_name, parent_entity_id,
               numbering_pattern, title_template, sort_order, is_active,
               created_at, updated_at, version
        FROM entity_definitions
        WHERE parent_entity_id = $1 AND kind = 'detail' AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        parent_entity_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 新增資料類別所需的資料。
#[derive(Debug, Clone)]
pub struct NewEntity<'a> {
    pub id: Uuid,
    pub key: &'a str,
    pub label: &'a str,
    pub icon: Option<&'a str>,
    pub kind: &'a str,
    pub storage: &'a str,
    /// 明細子表所屬的主表；`kind = detail` 時必須有值，其他為 `None`。
    pub parent_entity_id: Option<Uuid>,
    pub numbering_pattern: Option<&'a str>,
    pub title_template: Option<&'a str>,
    pub sort_order: i32,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

/// 新增一個啟用中的自訂資料類別（`is_system = false`）。
///
/// 代碼重複時回傳違反 UNIQUE 限制（[`ENTITY_KEY_UNIQUE`]）的 [`DbError::Sqlx`]。
pub async fn insert_entity<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewEntity<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO entity_definitions
            (id, key, label, icon, kind, is_system, storage, parent_entity_id, numbering_pattern,
             title_template, sort_order, created_at, created_by, updated_at, updated_by)
        VALUES ($1, $2, $3, $4, $5, false, $6, $12, $7, $8, $9, $10, $11, $10, $11)
        "#,
        new.id,
        new.key,
        new.label,
        new.icon,
        new.kind,
        new.storage,
        new.numbering_pattern,
        new.title_template,
        new.sort_order,
        new.created_at,
        new.created_by,
        new.parent_entity_id,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 修改資料類別的可修改屬性。
#[derive(Debug, Clone)]
pub struct EntityUpdate<'a> {
    pub id: Uuid,
    /// 讀取時的版本號。
    pub version: i32,
    pub label: &'a str,
    pub icon: Option<&'a str>,
    pub numbering_pattern: Option<&'a str>,
    pub title_template: Option<&'a str>,
    pub sort_order: i32,
    pub is_active: bool,
    pub updated_by: Uuid,
}

/// 修改資料類別並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_entity<'e>(
    executor: impl PgExecutor<'e>,
    update: &EntityUpdate<'_>,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE entity_definitions
        SET label = $3, icon = $4, numbering_pattern = $5, title_template = $6, sort_order = $7,
            is_active = $8, updated_by = $9, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        update.id,
        update.version,
        update.label,
        update.icon,
        update.numbering_pattern,
        update.title_template,
        update.sort_order,
        update.is_active,
        update.updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 修改資料類別的選單排序並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_entity_sort_order<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    sort_order: i32,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE entity_definitions
        SET sort_order = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        sort_order,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 軟刪除資料類別並遞增 `version`。版本號不符或已刪除時影響 0 筆。
///
/// 代碼的 UNIQUE 限制包含已刪除的資料類別，所以刪除後代碼不能再使用。
pub async fn soft_delete_entity<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    deleted_at: OffsetDateTime,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE entity_definitions
        SET deleted_at = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        deleted_at,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 引用某些資料類別或欄位的欄位：關聯欄位（[`list_relation_references`]）或公式欄位
/// （[`list_formula_field_references`]、[`list_formula_entity_references`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationReferenceRow {
    pub entity_key: String,
    pub entity_label: String,
    pub field_key: String,
    pub field_label: String,
}

/// 列出 `relation_entity_id` 為 `targets` 之一的有效（啟用、未刪除）關聯欄位，且所屬資料類別
/// 也有效、不在 `targets` 之中。依資料類別代碼、欄位代碼排序。
pub async fn list_relation_references<'e>(
    executor: impl PgExecutor<'e>,
    targets: &[Uuid],
) -> Result<Vec<RelationReferenceRow>, DbError> {
    let rows = sqlx::query_as!(
        RelationReferenceRow,
        r#"
        SELECT e.key AS entity_key, e.label AS entity_label,
               f.field_key, f.label AS field_label
        FROM field_definitions f
        JOIN entity_definitions e ON e.id = f.entity_id
        WHERE f.relation_entity_id = ANY($1)
          AND f.data_type = 'relation' AND f.is_active AND f.deleted_at IS NULL
          AND e.is_active AND e.deleted_at IS NULL
          AND NOT (e.id = ANY($1))
        ORDER BY e.key, f.field_key
        "#,
        targets,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// field_definitions
// ---------------------------------------------------------------------------

/// `field_definitions` 的資料列（不含 `deleted_at`、`created_by`、`updated_by`）。
#[derive(Debug, Clone, PartialEq)]
pub struct FieldRow {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub field_key: String,
    pub label: String,
    pub data_type: String,
    pub is_system: bool,
    pub storage: String,
    pub column_name: Option<String>,
    pub is_required: bool,
    pub is_visible: bool,
    pub is_readonly: bool,
    pub is_searchable: bool,
    pub is_unique: bool,
    pub default_value: Option<Value>,
    pub validation: Option<Value>,
    pub options: Option<Value>,
    pub decimal_places: Option<i16>,
    pub relation_entity_id: Option<Uuid>,
    pub formula: Option<String>,
    pub formula_result_type: Option<String>,
    pub ui_widget: Option<String>,
    pub column_width: Option<i32>,
    pub help_text: Option<String>,
    pub sort_order: i32,
    pub is_active: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub version: i32,
}

/// 列出資料類別所有未刪除的欄位（含已停用），依 `sort_order`、`id` 排序。
pub async fn list_fields<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
) -> Result<Vec<FieldRow>, DbError> {
    let rows = sqlx::query_as!(
        FieldRow,
        r#"
        SELECT id, entity_id, field_key, label, data_type, is_system, storage, column_name,
               is_required, is_visible, is_readonly, is_searchable, is_unique,
               default_value, validation, options, decimal_places, relation_entity_id,
               formula, formula_result_type, ui_widget, column_width, help_text,
               sort_order, is_active, created_at, updated_at, version
        FROM field_definitions
        WHERE entity_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        entity_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 同 [`list_fields`]，並鎖定這些列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn list_fields_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
) -> Result<Vec<FieldRow>, DbError> {
    let rows = sqlx::query_as!(
        FieldRow,
        r#"
        SELECT id, entity_id, field_key, label, data_type, is_system, storage, column_name,
               is_required, is_visible, is_readonly, is_searchable, is_unique,
               default_value, validation, options, decimal_places, relation_entity_id,
               formula, formula_result_type, ui_widget, column_width, help_text,
               sort_order, is_active, created_at, updated_at, version
        FROM field_definitions
        WHERE entity_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        entity_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 依資料類別與欄位代碼查詢未刪除的欄位（含已停用）。
pub async fn find_field<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    field_key: &str,
) -> Result<Option<FieldRow>, DbError> {
    let row = sqlx::query_as!(
        FieldRow,
        r#"
        SELECT id, entity_id, field_key, label, data_type, is_system, storage, column_name,
               is_required, is_visible, is_readonly, is_searchable, is_unique,
               default_value, validation, options, decimal_places, relation_entity_id,
               formula, formula_result_type, ui_widget, column_width, help_text,
               sort_order, is_active, created_at, updated_at, version
        FROM field_definitions
        WHERE entity_id = $1 AND field_key = $2 AND deleted_at IS NULL
        "#,
        entity_id,
        field_key,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_field`]，並鎖定該列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn find_field_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    field_key: &str,
) -> Result<Option<FieldRow>, DbError> {
    let row = sqlx::query_as!(
        FieldRow,
        r#"
        SELECT id, entity_id, field_key, label, data_type, is_system, storage, column_name,
               is_required, is_visible, is_readonly, is_searchable, is_unique,
               default_value, validation, options, decimal_places, relation_entity_id,
               formula, formula_result_type, ui_widget, column_width, help_text,
               sort_order, is_active, created_at, updated_at, version
        FROM field_definitions
        WHERE entity_id = $1 AND field_key = $2 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        entity_id,
        field_key,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 欄位的可設定屬性（新增與修改共用）。
#[derive(Debug, Clone)]
pub struct FieldAttributes<'a> {
    pub label: &'a str,
    pub is_required: bool,
    pub is_visible: bool,
    pub is_readonly: bool,
    pub is_searchable: bool,
    pub is_unique: bool,
    pub default_value: Option<&'a Value>,
    pub validation: Option<&'a Value>,
    pub options: Option<&'a Value>,
    pub decimal_places: Option<i16>,
    /// `formula` 的公式；其他型態為 `None`。
    pub formula: Option<&'a str>,
    /// `formula` 的結果型態；其他型態為 `None`。
    pub formula_result_type: Option<&'a str>,
    /// `formula` 的相依欄位（`hammer_formula::deps_to_json`）；其他型態為 `None`。
    pub formula_deps: Option<&'a Value>,
    pub ui_widget: Option<&'a str>,
    pub column_width: Option<i32>,
    pub help_text: Option<&'a str>,
    pub sort_order: i32,
}

/// 新增欄位所需的資料。
#[derive(Debug, Clone)]
pub struct NewField<'a> {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub field_key: &'a str,
    pub data_type: &'a str,
    /// `relation` 關聯的資料類別（建立後不可修改，所以不在 [`FieldAttributes`] 內）。
    pub relation_entity_id: Option<Uuid>,
    pub attrs: FieldAttributes<'a>,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

/// 新增一個啟用中的自訂欄位（`is_system = false`、`storage = json`）。
///
/// 欄位代碼重複時回傳違反 UNIQUE 限制（[`FIELD_KEY_UNIQUE`]）的 [`DbError::Sqlx`]。
pub async fn insert_field<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewField<'_>,
) -> Result<(), DbError> {
    let a = &new.attrs;
    sqlx::query!(
        r#"
        INSERT INTO field_definitions
            (id, entity_id, field_key, label, data_type, is_system, storage,
             is_required, is_visible, is_readonly, is_searchable, is_unique,
             default_value, validation, options, decimal_places, ui_widget, column_width,
             help_text, sort_order, created_at, created_by, updated_at, updated_by,
             relation_entity_id, formula, formula_result_type, formula_deps)
        VALUES ($1, $2, $3, $4, $5, false, 'json', $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $19, $20, $21,
                $22, $23, $24)
        "#,
        new.id,
        new.entity_id,
        new.field_key,
        a.label,
        new.data_type,
        a.is_required,
        a.is_visible,
        a.is_readonly,
        a.is_searchable,
        a.is_unique,
        a.default_value,
        a.validation,
        a.options,
        a.decimal_places,
        a.ui_widget,
        a.column_width,
        a.help_text,
        a.sort_order,
        new.created_at,
        new.created_by,
        new.relation_entity_id,
        a.formula,
        a.formula_result_type,
        a.formula_deps,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 修改欄位的可設定屬性（含公式）與 `is_active`，並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_field<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    attrs: &FieldAttributes<'_>,
    is_active: bool,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let a = attrs;
    let result = sqlx::query!(
        r#"
        UPDATE field_definitions
        SET label = $3, is_required = $4, is_visible = $5, is_readonly = $6,
            is_searchable = $7, is_unique = $8, default_value = $9, validation = $10,
            options = $11, decimal_places = $12, ui_widget = $13, column_width = $14,
            help_text = $15, sort_order = $16, is_active = $17,
            updated_by = $18, formula = $19, formula_result_type = $20, formula_deps = $21,
            version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        a.label,
        a.is_required,
        a.is_visible,
        a.is_readonly,
        a.is_searchable,
        a.is_unique,
        a.default_value,
        a.validation,
        a.options,
        a.decimal_places,
        a.ui_widget,
        a.column_width,
        a.help_text,
        a.sort_order,
        is_active,
        updated_by,
        a.formula,
        a.formula_result_type,
        a.formula_deps,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 一個公式欄位的相依（`formula_deps`）。
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaDepsRow {
    pub entity_id: Uuid,
    pub field_key: String,
    pub formula_deps: Option<Value>,
}

/// 列出 `entity_ids` 中有效（啟用、未刪除）的公式欄位與其 `formula_deps`，
/// 依資料類別、`sort_order`、`id` 排序。
pub async fn list_formula_deps<'e>(
    executor: impl PgExecutor<'e>,
    entity_ids: &[Uuid],
) -> Result<Vec<FormulaDepsRow>, DbError> {
    let rows = sqlx::query_as!(
        FormulaDepsRow,
        r#"
        SELECT entity_id, field_key, formula_deps
        FROM field_definitions
        WHERE entity_id = ANY($1) AND data_type = 'formula'
          AND is_active AND deleted_at IS NULL
        ORDER BY entity_id, sort_order, id
        "#,
        entity_ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 列出引用欄位（資料類別 `entity_id` 的 `field_key`）的有效公式欄位：公式欄位啟用、未刪除，
/// 所屬資料類別未刪除（含已停用，重新啟用後公式仍需要這些欄位）；不含欄位本身。
/// 依資料類別代碼、欄位代碼排序。
///
/// 引用的方式（`formula_deps` 的元素，見 architecture.md §8.5）：
///
/// - `{"kind":"field"}`：同一個資料類別的欄位
/// - `{"kind":"relation"}`：同一個資料類別的關聯欄位本身（`relation`），或關聯欄位目標資料類別的
///   欄位（`field`）
/// - `{"kind":"detail"}`：所屬資料類別的明細子表（代碼為 `detail`）的欄位
pub async fn list_formula_field_references<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    field_key: &str,
) -> Result<Vec<RelationReferenceRow>, DbError> {
    let rows = sqlx::query_as!(
        RelationReferenceRow,
        r#"
        SELECT DISTINCT e.key AS entity_key, e.label AS entity_label,
               f.field_key, f.label AS field_label
        FROM field_definitions f
        JOIN entity_definitions e ON e.id = f.entity_id AND e.deleted_at IS NULL
        CROSS JOIN LATERAL jsonb_array_elements(
            CASE WHEN jsonb_typeof(f.formula_deps) = 'array' THEN f.formula_deps ELSE '[]' END
        ) AS d(dep)
        WHERE f.data_type = 'formula' AND f.is_active AND f.deleted_at IS NULL
          AND NOT (f.entity_id = $1 AND f.field_key = $2)
          AND (
            (d.dep->>'kind' = 'field' AND f.entity_id = $1 AND d.dep->>'field' = $2)
            OR (d.dep->>'kind' = 'relation' AND f.entity_id = $1 AND d.dep->>'relation' = $2)
            OR (d.dep->>'kind' = 'relation' AND d.dep->>'field' = $2 AND EXISTS (
                SELECT 1 FROM field_definitions rf
                WHERE rf.entity_id = f.entity_id AND rf.field_key = d.dep->>'relation'
                  AND rf.data_type = 'relation' AND rf.relation_entity_id = $1
                  AND rf.deleted_at IS NULL
            ))
            OR (d.dep->>'kind' = 'detail' AND d.dep->>'field' = $2 AND EXISTS (
                SELECT 1 FROM entity_definitions de
                WHERE de.id = $1 AND de.parent_entity_id = f.entity_id
                  AND de.key = d.dep->>'detail'
            ))
          )
        ORDER BY e.key, f.field_key
        "#,
        entity_id,
        field_key,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 列出引用資料類別 `targets`（停用明細子表、刪除資料類別時）的有效公式欄位，不含 `targets`
/// 本身的公式欄位：以子表彙總引用明細子表的欄位，或以關聯引用目標資料類別的欄位。
/// 公式欄位與所屬資料類別的條件同 [`list_formula_field_references`]。依資料類別代碼、欄位代碼排序。
pub async fn list_formula_entity_references<'e>(
    executor: impl PgExecutor<'e>,
    targets: &[Uuid],
) -> Result<Vec<RelationReferenceRow>, DbError> {
    let rows = sqlx::query_as!(
        RelationReferenceRow,
        r#"
        SELECT DISTINCT e.key AS entity_key, e.label AS entity_label,
               f.field_key, f.label AS field_label
        FROM field_definitions f
        JOIN entity_definitions e ON e.id = f.entity_id AND e.deleted_at IS NULL
        CROSS JOIN LATERAL jsonb_array_elements(
            CASE WHEN jsonb_typeof(f.formula_deps) = 'array' THEN f.formula_deps ELSE '[]' END
        ) AS d(dep)
        WHERE f.data_type = 'formula' AND f.is_active AND f.deleted_at IS NULL
          AND NOT (f.entity_id = ANY($1))
          AND (
            (d.dep->>'kind' = 'relation' AND EXISTS (
                SELECT 1 FROM field_definitions rf
                WHERE rf.entity_id = f.entity_id AND rf.field_key = d.dep->>'relation'
                  AND rf.data_type = 'relation' AND rf.relation_entity_id = ANY($1)
                  AND rf.deleted_at IS NULL
            ))
            OR (d.dep->>'kind' = 'detail' AND EXISTS (
                SELECT 1 FROM entity_definitions de
                WHERE de.id = ANY($1) AND de.parent_entity_id = f.entity_id
                  AND de.key = d.dep->>'detail'
            ))
          )
        ORDER BY e.key, f.field_key
        "#,
        targets,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 變更欄位型態所需的資料（§7.4）。
#[derive(Debug, Clone)]
pub struct FieldTypeUpdate<'a> {
    pub id: Uuid,
    pub version: i32,
    pub data_type: &'a str,
    pub decimal_places: Option<i16>,
    pub options: Option<&'a Value>,
    pub validation: Option<&'a Value>,
    pub default_value: Option<&'a Value>,
    /// 新型態不是 `relation` 時為 `None`（清除）。
    pub relation_entity_id: Option<Uuid>,
    pub updated_by: Uuid,
}

/// 修改欄位的 `data_type` 與相關設定，並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_field_type<'e>(
    executor: impl PgExecutor<'e>,
    update: &FieldTypeUpdate<'_>,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE field_definitions
        SET data_type = $3, decimal_places = $4, options = $5, validation = $6,
            default_value = $7, updated_by = $8, relation_entity_id = $9, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        update.id,
        update.version,
        update.data_type,
        update.decimal_places,
        update.options,
        update.validation,
        update.default_value,
        update.updated_by,
        update.relation_entity_id,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 只修改欄位的 `sort_order`，並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_field_sort_order<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    sort_order: i32,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE field_definitions
        SET sort_order = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        sort_order,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// schema_change_logs
// ---------------------------------------------------------------------------

/// 新增定義變更紀錄所需的資料。
#[derive(Debug, Clone)]
pub struct NewSchemaChangeLog<'a> {
    pub id: Uuid,
    pub changed_at: OffsetDateTime,
    pub changed_by: Uuid,
    /// `entity` / `field`
    pub target_type: &'a str,
    pub target_id: Uuid,
    /// `create` / `update` / `deactivate`
    pub action: &'a str,
    pub before_data: Option<&'a Value>,
    pub after_data: Option<&'a Value>,
}

/// 新增一筆定義變更紀錄。
pub async fn insert_schema_change_log<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewSchemaChangeLog<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO schema_change_logs
            (id, changed_at, changed_by, target_type, target_id, action, before_data, after_data)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
        new.id,
        new.changed_at,
        new.changed_by,
        new.target_type,
        new.target_id,
        new.action,
        new.before_data,
        new.after_data,
    )
    .execute(executor)
    .await?;
    Ok(())
}
