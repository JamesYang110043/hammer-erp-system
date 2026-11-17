//! `custom_records` 的存取（docs/architecture.md §7.1、§7.5、§8.5、§9.4）。
//!
//! - 單筆：[`find_record`]、[`find_record_for_update`]、[`insert_record`]、[`update_record_data`]、
//!   [`soft_delete_record`]
//! - 列表：[`list_records`]、[`count_records`]，條件為 [`RecordListFilter`]
//! - 唯一性：[`value_taken`]、[`line_value_taken`]；單號不可重複的索引為 [`RECORD_NO_UNIQUE`]
//! - 明細（`parent_record_id` 有值的資料）：[`list_lines`]、[`list_lines_for_update`]、
//!   [`update_line`]、[`soft_delete_lines`]、[`soft_delete_child_records`]；新增也用 [`insert_record`]
//! - 欄位型態變更（含已軟刪除的資料）：[`list_all_record_data_for_update`]、[`update_records_data`]
//! - 關聯欄位（§7.3）：[`lock_existing_records`]（寫入時檢查目標）、[`find_records_by_ids`]
//!   （回應的關聯資訊，含已軟刪除）、[`list_records_before`]（選擇器搜尋）
//!
//! 除了特別註明「含已軟刪除」的函式，已軟刪除（`deleted_at IS NOT NULL`）的資料一律視為不存在。
//! 欄位代碼與比較值一律以 bind 參數傳入，不拼接進 SQL；欄位型態檢查與值的格式檢查在
//! `hammer_core::records`，這裡只負責依 [`JsonCast`] 轉型後比較。

use serde_json::Value;
use sqlx::postgres::PgQueryResult;
use sqlx::{PgExecutor, Postgres, QueryBuilder};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;
use crate::system_record::SystemTable;

/// 同一個資料類別的單號不可重複的部分唯一索引（含已軟刪除的資料）。
pub const RECORD_NO_UNIQUE: &str = "uq_custom_records_entity_record_no";

/// `custom_records` 的資料列（不含 `parent_record_id`、`deleted_at`）。
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RecordRow {
    pub id: Uuid,
    pub entity_id: Uuid,
    /// 單號；資料類別沒有設定單號規則時為 `None`。
    pub record_no: Option<String>,
    pub data: Value,
    /// 公式欄位的計算錯誤：鍵為欄位代碼、值為錯誤原因；沒有錯誤時為 `None`（§7.6）。
    pub formula_errors: Option<Value>,
    pub created_at: OffsetDateTime,
    pub created_by: Option<Uuid>,
    pub updated_at: OffsetDateTime,
    pub updated_by: Option<Uuid>,
    pub version: i32,
}

/// 依資料類別與 id 查詢未刪除的資料。
pub async fn find_record<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    id: Uuid,
) -> Result<Option<RecordRow>, DbError> {
    let row = sqlx::query_as!(
        RecordRow,
        r#"
        SELECT id, entity_id, record_no, data, formula_errors, created_at, created_by,
               updated_at, updated_by, version
        FROM custom_records
        WHERE id = $1 AND entity_id = $2 AND deleted_at IS NULL
        "#,
        id,
        entity_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_record`]，並鎖定該列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn find_record_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    id: Uuid,
) -> Result<Option<RecordRow>, DbError> {
    let row = sqlx::query_as!(
        RecordRow,
        r#"
        SELECT id, entity_id, record_no, data, formula_errors, created_at, created_by,
               updated_at, updated_by, version
        FROM custom_records
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

/// 新增資料所需的資料。
#[derive(Debug, Clone)]
pub struct NewRecord<'a> {
    pub id: Uuid,
    pub entity_id: Uuid,
    /// 依單號規則產生的單號；沒有規則時為 `None`。
    pub record_no: Option<&'a str>,
    /// 明細所屬的主表資料；不是明細時為 `None`。
    pub parent_record_id: Option<Uuid>,
    /// 明細的順序；不是明細時為 0。
    pub sort_order: i32,
    /// JSON 物件。
    pub data: &'a Value,
    /// 公式欄位的計算錯誤（JSON 物件）；沒有錯誤時為 `None`。
    pub formula_errors: Option<&'a Value>,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

/// 新增一筆資料。違反「不可重複」的唯一索引（§7.5）或單號重複（[`RECORD_NO_UNIQUE`]）時
/// 回傳違反 UNIQUE 限制的 [`DbError::Sqlx`]。
pub async fn insert_record<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewRecord<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO custom_records
            (id, entity_id, record_no, parent_record_id, sort_order, data, formula_errors,
             created_at, created_by, updated_at, updated_by)
        VALUES ($1, $2, $3, $7, $8, $4, $9, $5, $6, $5, $6)
        "#,
        new.id,
        new.entity_id,
        new.record_no,
        new.data,
        new.created_at,
        new.created_by,
        new.parent_record_id,
        new.sort_order,
        new.formula_errors,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 取代資料的 `data`、`formula_errors` 並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_record_data<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    data: &Value,
    formula_errors: Option<&Value>,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records
        SET data = $3, formula_errors = $5, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        data,
        updated_by,
        formula_errors,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 軟刪除資料並遞增 `version`。版本號不符時影響 0 筆。
pub async fn soft_delete_record<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    deleted_at: OffsetDateTime,
    deleted_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records
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

/// 同一個資料類別中，是否有其他未刪除的資料在 `field_key` 有相同的值（`id` 為 `exclude_id` 的除外）。
pub async fn value_taken<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    exclude_id: Uuid,
    field_key: &str,
    value: &Value,
) -> Result<bool, DbError> {
    let taken = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM custom_records
            WHERE entity_id = $1 AND id <> $2 AND deleted_at IS NULL
              AND data -> $3 = $4
        ) AS "taken!"
        "#,
        entity_id,
        exclude_id,
        field_key,
        value,
    )
    .fetch_one(executor)
    .await?;
    Ok(taken)
}

// ---------------------------------------------------------------------------
// 明細
// ---------------------------------------------------------------------------

/// 明細的資料列（`parent_record_id` 有值的 `custom_records`，不含 `deleted_at`）。
#[derive(Debug, Clone, PartialEq)]
pub struct LineRow {
    pub id: Uuid,
    /// 明細子表（`kind = detail` 的資料類別）。
    pub entity_id: Uuid,
    pub parent_record_id: Uuid,
    pub sort_order: i32,
    pub data: Value,
    /// 同 [`RecordRow::formula_errors`]。
    pub formula_errors: Option<Value>,
    pub created_at: OffsetDateTime,
    pub created_by: Option<Uuid>,
    pub updated_at: OffsetDateTime,
    pub updated_by: Option<Uuid>,
    pub version: i32,
}

impl LineRow {
    /// 轉成 [`RecordRow`]（明細沒有單號）。
    pub fn into_record_row(self) -> RecordRow {
        RecordRow {
            id: self.id,
            entity_id: self.entity_id,
            record_no: None,
            data: self.data,
            formula_errors: self.formula_errors,
            created_at: self.created_at,
            created_by: self.created_by,
            updated_at: self.updated_at,
            updated_by: self.updated_by,
            version: self.version,
        }
    }
}

/// 主表資料 `parent_record_id` 的所有未刪除明細（不分明細子表），依明細子表、`sort_order`、`id` 排序。
pub async fn list_lines<'e>(
    executor: impl PgExecutor<'e>,
    parent_record_id: Uuid,
) -> Result<Vec<LineRow>, DbError> {
    let rows = sqlx::query_as!(
        LineRow,
        r#"
        SELECT id, entity_id, parent_record_id AS "parent_record_id!", sort_order, data,
               formula_errors, created_at, created_by, updated_at, updated_by, version
        FROM custom_records
        WHERE parent_record_id = $1 AND deleted_at IS NULL
        ORDER BY entity_id, sort_order, id
        "#,
        parent_record_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 主表資料 `parent_record_id` 在明細子表 `entity_id` 的未刪除明細，依 `sort_order`、`id` 排序，
/// 並鎖定這些列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn list_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    parent_record_id: Uuid,
) -> Result<Vec<LineRow>, DbError> {
    let rows = sqlx::query_as!(
        LineRow,
        r#"
        SELECT id, entity_id, parent_record_id AS "parent_record_id!", sort_order, data,
               formula_errors, created_at, created_by, updated_at, updated_by, version
        FROM custom_records
        WHERE entity_id = $1 AND parent_record_id = $2 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        entity_id,
        parent_record_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 取代明細的 `data`、`formula_errors` 與 `sort_order` 並遞增 `version`。明細的樂觀鎖定由主表的
/// `version` 負責，這裡不檢查版本；呼叫前必須已用 [`list_lines_for_update`] 鎖定。回傳影響的筆數。
pub async fn update_line<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    sort_order: i32,
    data: &Value,
    formula_errors: Option<&Value>,
    updated_by: Uuid,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records
        SET data = $3, formula_errors = $5, sort_order = $2, updated_by = $4,
            version = version + 1
        WHERE id = $1 AND parent_record_id IS NOT NULL AND deleted_at IS NULL
        "#,
        id,
        sort_order,
        data,
        updated_by,
        formula_errors,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// 軟刪除多筆明細並遞增 `version`。回傳影響的筆數。
pub async fn soft_delete_lines<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
    deleted_at: OffsetDateTime,
    deleted_by: Uuid,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records
        SET deleted_at = $2, updated_by = $3, version = version + 1
        WHERE id = ANY($1) AND parent_record_id IS NOT NULL AND deleted_at IS NULL
        "#,
        ids,
        deleted_at,
        deleted_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// 軟刪除主表資料 `parent_record_id` 的所有未刪除明細（不分明細子表）。回傳影響的筆數。
pub async fn soft_delete_child_records<'e>(
    executor: impl PgExecutor<'e>,
    parent_record_id: Uuid,
    deleted_at: OffsetDateTime,
    deleted_by: Uuid,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records
        SET deleted_at = $2, updated_by = $3, version = version + 1
        WHERE parent_record_id = $1 AND deleted_at IS NULL
        "#,
        parent_record_id,
        deleted_at,
        deleted_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// 明細子表 `entity_id` 中，**不屬於**主表資料 `parent_record_id` 的未刪除明細，是否有在
/// `field_key` 有相同的值。用於明細違反「不可重複」時找出重複的欄位（同一筆主表內的重複在寫入前檢查）。
pub async fn line_value_taken<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    parent_record_id: Uuid,
    field_key: &str,
    value: &Value,
) -> Result<bool, DbError> {
    let taken = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM custom_records
            WHERE entity_id = $1 AND parent_record_id IS DISTINCT FROM $2 AND deleted_at IS NULL
              AND data -> $3 = $4
        ) AS "taken!"
        "#,
        entity_id,
        parent_record_id,
        field_key,
        value,
    )
    .fetch_one(executor)
    .await?;
    Ok(taken)
}

// ---------------------------------------------------------------------------
// 關聯欄位
// ---------------------------------------------------------------------------

/// `ids` 中屬於資料類別 `entity_id` 且未刪除的資料 id，並以 `FOR SHARE` 鎖定這些列，
/// 避免在寫入關聯的 transaction 結束前被刪除。必須在 transaction 內呼叫。
pub async fn lock_existing_records<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    let rows = sqlx::query_scalar!(
        r#"
        SELECT id FROM custom_records
        WHERE entity_id = $1 AND id = ANY($2) AND deleted_at IS NULL
        ORDER BY id
        FOR SHARE
        "#,
        entity_id,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 關聯欄位所指的資料（**含已軟刪除**）。
#[derive(Debug, Clone, PartialEq)]
pub struct RelatedRecordRow {
    pub id: Uuid,
    pub record_no: Option<String>,
    pub data: Value,
    /// 已軟刪除。
    pub deleted: bool,
}

/// 依 id 查詢資料類別 `entity_id` 的資料（**含已軟刪除**），供回應的關聯資訊使用。
pub async fn find_records_by_ids<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<RelatedRecordRow>, DbError> {
    let rows = sqlx::query_as!(
        RelatedRecordRow,
        r#"
        SELECT id, record_no, data, deleted_at IS NOT NULL AS "deleted!"
        FROM custom_records
        WHERE entity_id = $1 AND id = ANY($2)
        "#,
        entity_id,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 選擇器搜尋的一批資料：未刪除、依 `id` 由新到舊，`before` 有值時只取 id 小於它的資料。
pub async fn list_records_before<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    before: Option<Uuid>,
    limit: i64,
) -> Result<Vec<RecordRow>, DbError> {
    let rows = sqlx::query_as!(
        RecordRow,
        r#"
        SELECT id, entity_id, record_no, data, formula_errors, created_at, created_by,
               updated_at, updated_by, version
        FROM custom_records
        WHERE entity_id = $1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR id < $2)
        ORDER BY id DESC
        LIMIT $3
        "#,
        entity_id,
        before,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// 欄位型態變更
// ---------------------------------------------------------------------------

/// 資料的 `data`（含已軟刪除的資料）。
#[derive(Debug, Clone, PartialEq)]
pub struct RecordDataRow {
    pub id: Uuid,
    pub data: Value,
    /// 已軟刪除。
    pub deleted: bool,
}

/// 列出資料類別的所有資料（**含已軟刪除**）的 `data`，依 `id` 排序，並鎖定這些列（`FOR UPDATE`）。
/// 必須在 transaction 內呼叫。
pub async fn list_all_record_data_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
) -> Result<Vec<RecordDataRow>, DbError> {
    let rows = sqlx::query_as!(
        RecordDataRow,
        r#"
        SELECT id, data, deleted_at IS NOT NULL AS "deleted!"
        FROM custom_records
        WHERE entity_id = $1
        ORDER BY id
        FOR UPDATE
        "#,
        entity_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 一次取代多筆資料（**含已軟刪除**）的 `data`，並遞增 `version`。回傳影響的筆數。
///
/// `ids` 與 `data` 依序對應，長度必須相同。
pub async fn update_records_data<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    ids: &[Uuid],
    data: &[Value],
    updated_by: Uuid,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records AS r
        SET data = u.data, updated_by = $4, version = r.version + 1
        FROM UNNEST($2::uuid[], $3::jsonb[]) AS u(id, data)
        WHERE r.id = u.id AND r.entity_id = $1
        "#,
        entity_id,
        ids,
        data,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------
// 重新計算公式
// ---------------------------------------------------------------------------

/// 重新計算的一批主表資料：資料類別 `entity_id` 未刪除的資料，依 `id` 由小到大，`after` 有值時只取
/// id 大於它的資料，並鎖定這些列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn list_records_after_for_update<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<RecordRow>, DbError> {
    let rows = sqlx::query_as!(
        RecordRow,
        r#"
        SELECT id, entity_id, record_no, data, formula_errors, created_at, created_by,
               updated_at, updated_by, version
        FROM custom_records
        WHERE entity_id = $1 AND parent_record_id IS NULL AND deleted_at IS NULL
          AND ($2::uuid IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        FOR UPDATE
        "#,
        entity_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 多筆主表資料的所有未刪除明細（不分明細子表），依主表、明細子表、`sort_order`、`id` 排序，
/// 並鎖定這些列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn list_lines_of_parents_for_update<'e>(
    executor: impl PgExecutor<'e>,
    parent_record_ids: &[Uuid],
) -> Result<Vec<LineRow>, DbError> {
    let rows = sqlx::query_as!(
        LineRow,
        r#"
        SELECT id, entity_id, parent_record_id AS "parent_record_id!", sort_order, data,
               formula_errors, created_at, created_by, updated_at, updated_by, version
        FROM custom_records
        WHERE parent_record_id = ANY($1) AND deleted_at IS NULL
        ORDER BY parent_record_id, entity_id, sort_order, id
        FOR UPDATE
        "#,
        parent_record_ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 一次取代多筆未刪除資料（主表或明細）的 `data` 與 `formula_errors`，並遞增 `version`。不檢查版本，
/// 呼叫前必須已鎖定這些列。回傳影響的筆數。
///
/// `ids`、`data`、`formula_errors` 依序對應，長度必須相同；`formula_errors` 為 JSON `null` 時寫入
/// 資料庫 `NULL`（沒有錯誤）。
pub async fn update_formula_values<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
    data: &[Value],
    formula_errors: &[Value],
    updated_by: Uuid,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records AS r
        SET data = u.data, formula_errors = NULLIF(u.formula_errors, 'null'::jsonb),
            updated_by = $4, version = r.version + 1
        FROM UNNEST($1::uuid[], $2::jsonb[], $3::jsonb[]) AS u(id, data, formula_errors)
        WHERE r.id = u.id AND r.deleted_at IS NULL
        "#,
        ids,
        data,
        formula_errors,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------
// 列表
// ---------------------------------------------------------------------------

/// 取出 `data` 的欄位值後的轉型方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonCast {
    /// `data ->> key`
    Text,
    /// `(data ->> key)::numeric`，用於 `integer`、`decimal`、`money`
    Numeric,
    /// `(data ->> key)::date`
    Date,
    /// `(data ->> key)::timestamptz`
    Timestamptz,
    /// `(data ->> key)::boolean`
    Boolean,
    /// `data -> key`（JSONB 本身，只用於排序陣列等不轉型的值）
    Jsonb,
}

impl JsonCast {
    /// SQL 型別名稱；`Text`、`Jsonb` 不轉型。
    fn sql_type(self) -> Option<&'static str> {
        match self {
            Self::Text | Self::Jsonb => None,
            Self::Numeric => Some("numeric"),
            Self::Date => Some("date"),
            Self::Timestamptz => Some("timestamptz"),
            Self::Boolean => Some("boolean"),
        }
    }
}

/// 比較運算子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

impl CompareOp {
    fn sql(self) -> &'static str {
        match self {
            Self::Eq => " = ",
            Self::Gt => " > ",
            Self::Gte => " >= ",
            Self::Lt => " < ",
            Self::Lte => " <= ",
        }
    }
}

/// 一個篩選條件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordCondition {
    /// 轉型後比較：`(data ->> key)::cast <op> value::cast`。`value` 必須是可轉型的文字；
    /// `cast` 不可為 [`JsonCast::Jsonb`]。
    Compare {
        field_key: String,
        cast: JsonCast,
        op: CompareOp,
        value: String,
    },
    /// 文字包含（不分大小寫，`ILIKE`）。`text` 中的 `%`、`_`、`\` 視為一般字元。
    Contains { field_key: String, text: String },
    /// `has_value = true`：`data` 有此鍵；`false`：沒有此鍵。
    HasValue { field_key: String, has_value: bool },
    /// JSON 陣列包含任一個字串（`?|`）。
    ContainsAny {
        field_key: String,
        values: Vec<String>,
    },
    /// 單號等於（`record_no = value`）。
    RecordNoEq(String),
    /// 單號包含（不分大小寫，`ILIKE`）。`%`、`_`、`\` 視為一般字元。
    RecordNoContains(String),
}

/// 排序的對象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderTarget {
    /// `data` 的欄位，依 `cast` 轉型後比較。
    Field { field_key: String, cast: JsonCast },
    /// 單號（`record_no`，文字比較）。
    RecordNo,
}

/// 排序方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordOrder {
    pub target: OrderTarget,
    pub descending: bool,
}

/// 列表的查詢條件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordListFilter {
    pub entity_id: Uuid,
    /// 全部須符合（AND）。
    pub conditions: Vec<RecordCondition>,
    /// `None`：依 `id` 由新到舊。
    pub order: Option<RecordOrder>,
}

/// 依條件分頁查詢資料。
///
/// 有排序欄位時：`ORDER BY <值> <方向> NULLS LAST, id <方向>`；沒有時：`ORDER BY id DESC`。
pub async fn list_records<'e>(
    executor: impl PgExecutor<'e>,
    filter: &RecordListFilter,
    limit: i64,
    offset: i64,
) -> Result<Vec<RecordRow>, DbError> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT id, entity_id, record_no, data, formula_errors, created_at, created_by, \
         updated_at, updated_by, version FROM custom_records",
    );
    push_where(&mut qb, filter);
    push_order(&mut qb, Source::CustomRecords, filter.order.as_ref());
    qb.push(" LIMIT ").push_bind(limit);
    qb.push(" OFFSET ").push_bind(offset);
    let rows = qb.build_query_as::<RecordRow>().fetch_all(executor).await?;
    Ok(rows)
}

/// 符合條件的資料筆數。條件與 [`list_records`] 相同。
pub async fn count_records<'e>(
    executor: impl PgExecutor<'e>,
    filter: &RecordListFilter,
) -> Result<i64, DbError> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM custom_records");
    push_where(&mut qb, filter);
    let count = qb.build_query_scalar::<i64>().fetch_one(executor).await?;
    Ok(count)
}

fn push_where(qb: &mut QueryBuilder<Postgres>, filter: &RecordListFilter) {
    qb.push(" WHERE deleted_at IS NULL AND entity_id = ")
        .push_bind(filter.entity_id);
    push_conditions(qb, Source::CustomRecords, &filter.conditions);
}

/// 篩選與排序的資料來源：欄位值從哪裡取出。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Source<'a> {
    /// `custom_records`：所有欄位都在 `data`。
    CustomRecords,
    /// 系統資料表：系統欄位是實體欄位，其他欄位在 `custom_data`；沒有單號。
    Table(&'a SystemTable),
}

impl<'a> Source<'a> {
    /// 存放 JSON 欄位的實體欄位。
    fn json_column(self) -> &'static str {
        match self {
            Self::CustomRecords => "data",
            Self::Table(_) => "custom_data",
        }
    }

    /// 單號的運算式；系統資料表沒有單號，一律為 `NULL`。
    fn record_no(self) -> &'static str {
        match self {
            Self::CustomRecords => "record_no",
            Self::Table(_) => "NULL::text",
        }
    }

    /// `field_key` 是否為實體欄位（只有系統資料表的系統欄位）。欄位名稱已在
    /// [`SystemTable::new`] 檢查過，可以直接拼接。
    fn column(self, field_key: &str) -> Option<&'a str> {
        match self {
            Self::CustomRecords => None,
            Self::Table(table) => table.column(field_key).map(|c| c.name.as_str()),
        }
    }
}

/// 在 `WHERE` 之後加上 `AND <條件>`（全部須符合）。
pub(crate) fn push_conditions(
    qb: &mut QueryBuilder<Postgres>,
    source: Source<'_>,
    conditions: &[RecordCondition],
) {
    for condition in conditions {
        qb.push(" AND ");
        match condition {
            RecordCondition::Compare {
                field_key,
                cast,
                op,
                value,
            } => {
                push_value(qb, source, field_key, *cast);
                qb.push(op.sql()).push_bind(value.clone());
                if let Some(t) = cast.sql_type() {
                    qb.push("::").push(t);
                }
            }
            RecordCondition::Contains { field_key, text } => {
                push_value(qb, source, field_key, JsonCast::Text);
                qb.push(" ILIKE ")
                    .push_bind(format!("%{}%", escape_like(text)))
                    .push(" ESCAPE '\\'");
            }
            RecordCondition::HasValue {
                field_key,
                has_value,
            } => match source.column(field_key) {
                Some(column) => {
                    let not = if *has_value { " NOT" } else { "" };
                    qb.push("(\"")
                        .push(column)
                        .push("\" IS")
                        .push(not)
                        .push(" NULL)");
                }
                None => {
                    if !has_value {
                        qb.push("NOT ");
                    }
                    qb.push("(")
                        .push(source.json_column())
                        .push(" ? ")
                        .push_bind(field_key.clone())
                        .push(")");
                }
            },
            RecordCondition::ContainsAny { field_key, values } => {
                qb.push("(");
                push_value(qb, source, field_key, JsonCast::Jsonb);
                qb.push(" ?| ").push_bind(values.clone()).push("::text[])");
            }
            RecordCondition::RecordNoEq(value) => {
                qb.push(source.record_no())
                    .push(" = ")
                    .push_bind(value.clone());
            }
            RecordCondition::RecordNoContains(text) => {
                qb.push(source.record_no())
                    .push(" ILIKE ")
                    .push_bind(format!("%{}%", escape_like(text)))
                    .push(" ESCAPE '\\'");
            }
        }
    }
}

/// `ORDER BY`：有排序欄位時 `<值> <方向> NULLS LAST, id <方向>`；沒有時 `id DESC`。
pub(crate) fn push_order(
    qb: &mut QueryBuilder<Postgres>,
    source: Source<'_>,
    order: Option<&RecordOrder>,
) {
    match order {
        Some(order) => {
            let dir = if order.descending { " DESC" } else { " ASC" };
            qb.push(" ORDER BY ");
            match &order.target {
                OrderTarget::Field { field_key, cast } => push_value(qb, source, field_key, *cast),
                OrderTarget::RecordNo => {
                    qb.push(source.record_no());
                }
            }
            qb.push(dir).push(" NULLS LAST, id").push(dir);
        }
        None => {
            qb.push(" ORDER BY id DESC");
        }
    }
}

/// 取出欄位值並轉型。JSON 欄位的代碼以 bind 參數傳入；實體欄位的名稱已檢查過，加上引號拼接。
fn push_value(
    qb: &mut QueryBuilder<Postgres>,
    source: Source<'_>,
    field_key: &str,
    cast: JsonCast,
) {
    if let Some(column) = source.column(field_key) {
        match (cast, cast.sql_type()) {
            (JsonCast::Jsonb, _) => qb.push("to_jsonb(\"").push(column).push("\")"),
            (_, None) => qb.push("(\"").push(column).push("\"::text)"),
            (_, Some(t)) => qb.push("(\"").push(column).push("\"::").push(t).push(")"),
        };
        return;
    }
    let json = source.json_column();
    match (cast, cast.sql_type()) {
        (JsonCast::Jsonb, _) => {
            qb.push("(")
                .push(json)
                .push(" -> ")
                .push_bind(field_key.to_owned())
                .push(")");
        }
        (_, None) => {
            qb.push("(")
                .push(json)
                .push(" ->> ")
                .push_bind(field_key.to_owned())
                .push(")");
        }
        (_, Some(t)) => {
            qb.push("((")
                .push(json)
                .push(" ->> ")
                .push_bind(field_key.to_owned())
                .push(")::")
                .push(t)
                .push(")");
        }
    }
}

/// 跳脫 `LIKE` 的特殊字元（`ESCAPE '\'`）。
pub(crate) fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_wildcards() {
        assert_eq!(escape_like("50%_off\\"), "50\\%\\_off\\\\");
        assert_eq!(escape_like("王小明"), "王小明");
    }

    #[test]
    fn builds_sql_with_bind_parameters_only() {
        let filter = RecordListFilter {
            entity_id: Uuid::nil(),
            conditions: vec![
                RecordCondition::Compare {
                    field_key: "qty'; DROP TABLE users; --".into(),
                    cast: JsonCast::Numeric,
                    op: CompareOp::Gte,
                    value: "1".into(),
                },
                RecordCondition::Contains {
                    field_key: "name".into(),
                    text: "a".into(),
                },
                RecordCondition::HasValue {
                    field_key: "note".into(),
                    has_value: false,
                },
                RecordCondition::ContainsAny {
                    field_key: "tags".into(),
                    values: vec!["a".into()],
                },
                RecordCondition::RecordNoEq("SO-001".into()),
                RecordCondition::RecordNoContains("0%".into()),
            ],
            order: None,
        };
        let mut qb = QueryBuilder::<Postgres>::new("SELECT 1 FROM custom_records");
        push_where(&mut qb, &filter);
        assert_eq!(
            qb.sql().as_str(),
            "SELECT 1 FROM custom_records WHERE deleted_at IS NULL AND entity_id = $1 \
             AND ((data ->> $2)::numeric) >= $3::numeric \
             AND (data ->> $4) ILIKE $5 ESCAPE '\\' \
             AND NOT (data ? $6) \
             AND ((data -> $7) ?| $8::text[]) \
             AND record_no = $9 \
             AND record_no ILIKE $10 ESCAPE '\\'"
        );
    }
}
