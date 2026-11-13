//! `custom_records` 的存取（docs/architecture.md §7.1、§7.5、§8.5、§9.4）。
//!
//! - 單筆：[`find_record`]、[`find_record_for_update`]、[`insert_record`]、[`update_record_data`]、
//!   [`soft_delete_record`]
//! - 列表：[`list_records`]、[`count_records`]，條件為 [`RecordListFilter`]
//! - 唯一性：[`value_taken`]；單號不可重複的索引為 [`RECORD_NO_UNIQUE`]
//! - 欄位型態變更（含已軟刪除的資料）：[`list_all_record_data_for_update`]、[`update_records_data`]
//!
//! 已軟刪除（`deleted_at IS NOT NULL`）的資料一律視為不存在。
//! 欄位代碼與比較值一律以 bind 參數傳入，不拼接進 SQL；欄位型態檢查與值的格式檢查在
//! `hammer_core::records`，這裡只負責依 [`JsonCast`] 轉型後比較。

use serde_json::Value;
use sqlx::postgres::PgQueryResult;
use sqlx::{PgExecutor, Postgres, QueryBuilder};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

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
        SELECT id, entity_id, record_no, data, created_at, created_by, updated_at, updated_by,
               version
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
        SELECT id, entity_id, record_no, data, created_at, created_by, updated_at, updated_by,
               version
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
    /// JSON 物件。
    pub data: &'a Value,
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
            (id, entity_id, record_no, data, created_at, created_by, updated_at, updated_by)
        VALUES ($1, $2, $3, $4, $5, $6, $5, $6)
        "#,
        new.id,
        new.entity_id,
        new.record_no,
        new.data,
        new.created_at,
        new.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 取代資料的 `data` 並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_record_data<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    data: &Value,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE custom_records
        SET data = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        data,
        updated_by,
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
        "SELECT id, entity_id, record_no, data, created_at, created_by, updated_at, updated_by, \
         version FROM custom_records",
    );
    push_where(&mut qb, filter);
    match &filter.order {
        Some(order) => {
            let dir = if order.descending { " DESC" } else { " ASC" };
            qb.push(" ORDER BY ");
            match &order.target {
                OrderTarget::Field { field_key, cast } => push_value(&mut qb, field_key, *cast),
                OrderTarget::RecordNo => {
                    qb.push("record_no");
                }
            }
            qb.push(dir).push(" NULLS LAST, id").push(dir);
        }
        None => {
            qb.push(" ORDER BY id DESC");
        }
    }
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
    for condition in &filter.conditions {
        qb.push(" AND ");
        match condition {
            RecordCondition::Compare {
                field_key,
                cast,
                op,
                value,
            } => {
                push_value(qb, field_key, *cast);
                qb.push(op.sql()).push_bind(value.clone());
                if let Some(t) = cast.sql_type() {
                    qb.push("::").push(t);
                }
            }
            RecordCondition::Contains { field_key, text } => {
                push_value(qb, field_key, JsonCast::Text);
                qb.push(" ILIKE ")
                    .push_bind(format!("%{}%", escape_like(text)))
                    .push(" ESCAPE '\\'");
            }
            RecordCondition::HasValue {
                field_key,
                has_value,
            } => {
                if !has_value {
                    qb.push("NOT ");
                }
                qb.push("(data ? ").push_bind(field_key.clone()).push(")");
            }
            RecordCondition::ContainsAny { field_key, values } => {
                qb.push("(data -> ")
                    .push_bind(field_key.clone())
                    .push(" ?| ")
                    .push_bind(values.clone())
                    .push("::text[])");
            }
            RecordCondition::RecordNoEq(value) => {
                qb.push("record_no = ").push_bind(value.clone());
            }
            RecordCondition::RecordNoContains(text) => {
                qb.push("record_no ILIKE ")
                    .push_bind(format!("%{}%", escape_like(text)))
                    .push(" ESCAPE '\\'");
            }
        }
    }
}

/// 取出欄位值並轉型。欄位代碼以 bind 參數傳入。
fn push_value(qb: &mut QueryBuilder<Postgres>, field_key: &str, cast: JsonCast) {
    match (cast, cast.sql_type()) {
        (JsonCast::Jsonb, _) => {
            qb.push("(data -> ")
                .push_bind(field_key.to_owned())
                .push(")");
        }
        (_, None) => {
            qb.push("(data ->> ")
                .push_bind(field_key.to_owned())
                .push(")");
        }
        (_, Some(t)) => {
            qb.push("((data ->> ")
                .push_bind(field_key.to_owned())
                .push(")::")
                .push(t)
                .push(")");
        }
    }
}

/// 跳脫 `LIKE` 的特殊字元（`ESCAPE '\'`）。
fn escape_like(text: &str) -> String {
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
             AND (data -> $7 ?| $8::text[]) \
             AND record_no = $9 \
             AND record_no ILIKE $10 ESCAPE '\\'"
        );
    }
}
