//! 系統資料類別的資料（`storage = table`）存取（docs/architecture.md §7.1、§7.5、§8.6、§9.4）。
//!
//! 系統資料表（例如 `units`）的系統欄位是實體欄位，自訂欄位存在 `custom_data`。這裡把兩者合成與
//! [`crate::record`] 相同的 [`RecordRow`]：`data` 以 `field_key` 為鍵，同時包含系統欄位與自訂欄位，
//! 所以上層（`hammer_core::records`）可以沿用同一套驗證、標題、操作紀錄。
//!
//! - 單筆：[`find_record`]、[`find_record_for_update`]、[`insert_record`]、[`update_record`]、
//!   [`soft_delete_record`]
//! - 列表：[`list_records`]、[`count_records`]（條件格式與 [`crate::record::RecordListFilter`] 相同）
//! - 選擇器：[`list_records_before`]（排除 `is_active = false`）
//! - 唯一性：[`value_taken`]
//!
//! # 安全
//!
//! 資料表與欄位名稱無法使用 bind 參數，必須拼接進 SQL。[`SystemTable::new`] 只接受符合
//! [`is_safe_identifier`] 的名稱（只含 `a-z`、`0-9`、`_`），拼接時一律加上引號；名稱來自 migration
//! 寫入的 `entity_definitions.table_name` 與 `field_definitions`（`storage = column`）。值與 JSON 欄位的
//! 代碼一律以 bind 參數傳入。
//!
//! # 值的格式
//!
//! - 讀取：沒有值（`NULL`）的實體欄位不出現在 `data`；`NUMERIC` 轉成文字（實體欄位的小數位數與欄位定義
//!   相同，所以已補齊小數位數，§7.3），其他型態依 PostgreSQL 的 JSON 轉換（`DATE` 為 `YYYY-MM-DD`、
//!   `UUID` 為小寫連字號格式）。
//! - 寫入：實體欄位的值從 request 的 `data`（JSON 物件）以 `data ->> 'field_key'` 取出後轉型；沒有值時
//!   寫入 `NULL`。`custom_data` 由上層分出，只包含自訂欄位。
//!
//! 已軟刪除（`deleted_at IS NOT NULL`）的資料一律視為不存在。系統資料類別沒有單號，`record_no` 一律為
//! `None`。

use serde_json::Value;
use sqlx::postgres::PgQueryResult;
use sqlx::{PgExecutor, Postgres, QueryBuilder};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;
use crate::dynamic_index::is_safe_identifier;
use crate::record::{RecordListFilter, RecordRow, Source, push_conditions, push_order};

/// 選擇器排除停用資料時使用的實體欄位（§7.1）。
pub const IS_ACTIVE_COLUMN: &str = "is_active";

/// 實體欄位的 SQL 型別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    /// `TEXT`（`text`、`long_text`、`select`）。
    Text,
    /// `INTEGER`。
    Integer,
    /// `NUMERIC`（`decimal`、`money`）。
    Numeric,
    /// `DATE`。
    Date,
    /// `TIMESTAMPTZ`。
    Timestamptz,
    /// `BOOLEAN`。
    Boolean,
    /// `UUID`（`relation`）。
    Uuid,
}

impl ColumnType {
    /// 寫入時的轉型；`Text` 不轉型。
    fn cast(self) -> Option<&'static str> {
        match self {
            Self::Text => None,
            Self::Integer => Some("integer"),
            Self::Numeric => Some("numeric"),
            Self::Date => Some("date"),
            Self::Timestamptz => Some("timestamptz"),
            Self::Boolean => Some("boolean"),
            Self::Uuid => Some("uuid"),
        }
    }
}

/// 一個實體欄位（系統欄位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemColumn {
    /// 欄位名稱，與 `field_key` 相同（§7.1）。
    pub name: String,
    pub column_type: ColumnType,
}

/// 一個系統資料類別的專屬資料表與其系統欄位。只能由 [`SystemTable::new`] 建立。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTable {
    entity_id: Uuid,
    name: String,
    columns: Vec<SystemColumn>,
}

impl SystemTable {
    /// 資料類別 `entity_id` 的資料表 `name` 與系統欄位 `columns`。資料表或欄位名稱不符合
    /// [`is_safe_identifier`] → [`DbError::UnsafeIdentifier`]。
    pub fn new(
        entity_id: Uuid,
        name: impl Into<String>,
        columns: Vec<SystemColumn>,
    ) -> Result<Self, DbError> {
        let name = name.into();
        for s in std::iter::once(&name).chain(columns.iter().map(|c| &c.name)) {
            if !is_safe_identifier(s) {
                return Err(DbError::UnsafeIdentifier(s.clone()));
            }
        }
        Ok(Self {
            entity_id,
            name,
            columns,
        })
    }

    /// 資料表名稱。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 名稱為 `field_key` 的系統欄位。
    pub fn column(&self, field_key: &str) -> Option<&SystemColumn> {
        self.columns.iter().find(|c| c.name == field_key)
    }

    /// `SELECT … FROM "<table>"`，欄位與 [`RecordRow`] 相同。
    fn push_select(&self, qb: &mut QueryBuilder<Postgres>) {
        qb.push("SELECT id, ")
            .push_bind(self.entity_id)
            .push("::uuid AS entity_id, NULL::text AS record_no, ");
        self.push_data(qb);
        qb.push(
            " AS data, formula_errors, created_at, created_by, updated_at, updated_by, version \
             FROM \"",
        )
        .push(&self.name)
        .push("\"");
    }

    /// 合成的 `data`：`custom_data` 加上有值的實體欄位（實體欄位優先）。
    fn push_data(&self, qb: &mut QueryBuilder<Postgres>) {
        if self.columns.is_empty() {
            qb.push("custom_data");
            return;
        }
        qb.push("(custom_data || jsonb_strip_nulls(jsonb_build_object(");
        for (i, c) in self.columns.iter().enumerate() {
            if i > 0 {
                qb.push(", ");
            }
            qb.push("'")
                .push(&c.name)
                .push("', \"")
                .push(&c.name)
                .push("\"");
            if c.column_type == ColumnType::Numeric {
                qb.push("::text");
            }
        }
        qb.push(")))");
    }

    /// 從子查詢 `v.d`（request 的 `data`）取出實體欄位的值並轉型。
    fn push_column_value(qb: &mut QueryBuilder<Postgres>, column: &SystemColumn) {
        qb.push("(v.d ->> '").push(&column.name).push("')");
        if let Some(cast) = column.column_type.cast() {
            qb.push("::").push(cast);
        }
    }
}

/// 依 id 查詢未刪除的資料。
pub async fn find_record<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    id: Uuid,
) -> Result<Option<RecordRow>, DbError> {
    let mut qb = find_query(table, id, false);
    Ok(qb
        .build_query_as::<RecordRow>()
        .fetch_optional(executor)
        .await?)
}

/// 同 [`find_record`]，並鎖定該列（`FOR UPDATE`）。必須在 transaction 內呼叫。
pub async fn find_record_for_update<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    id: Uuid,
) -> Result<Option<RecordRow>, DbError> {
    let mut qb = find_query(table, id, true);
    Ok(qb
        .build_query_as::<RecordRow>()
        .fetch_optional(executor)
        .await?)
}

fn find_query(table: &SystemTable, id: Uuid, for_update: bool) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new("");
    table.push_select(&mut qb);
    qb.push(" WHERE id = ")
        .push_bind(id)
        .push(" AND deleted_at IS NULL");
    if for_update {
        qb.push(" FOR UPDATE");
    }
    qb
}

/// 新增資料所需的資料。
#[derive(Debug, Clone)]
pub struct NewSystemRecord<'a> {
    pub id: Uuid,
    /// 完整的資料（JSON 物件），實體欄位的值從這裡取出。
    pub data: &'a Value,
    /// 自訂欄位（JSON 物件）。
    pub custom_data: &'a Value,
    /// 公式欄位的計算錯誤（JSON 物件）；沒有錯誤時為 `None`。
    pub formula_errors: Option<&'a Value>,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

/// 新增一筆資料。違反唯一索引時回傳違反 UNIQUE 限制的 [`DbError::Sqlx`]。
pub async fn insert_record<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    new: &NewSystemRecord<'_>,
) -> Result<(), DbError> {
    insert_query(table, new).build().execute(executor).await?;
    Ok(())
}

fn insert_query(table: &SystemTable, new: &NewSystemRecord<'_>) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new("INSERT INTO \"");
    qb.push(&table.name).push("\" (id");
    for c in &table.columns {
        qb.push(", \"").push(&c.name).push("\"");
    }
    qb.push(
        ", custom_data, formula_errors, created_at, created_by, updated_at, updated_by) SELECT ",
    )
    .push_bind(new.id);
    for c in &table.columns {
        qb.push(", ");
        SystemTable::push_column_value(&mut qb, c);
    }
    qb.push(", ")
        .push_bind(new.custom_data)
        .push(", ")
        .push_bind(new.formula_errors)
        .push(", ")
        .push_bind(new.created_at)
        .push(", ")
        .push_bind(new.created_by)
        .push(", ")
        .push_bind(new.created_at)
        .push(", ")
        .push_bind(new.created_by)
        .push(" FROM (SELECT ")
        .push_bind(new.data)
        .push("::jsonb AS d) AS v");
    qb
}

/// 修改資料所需的資料。
#[derive(Debug, Clone)]
pub struct SystemRecordUpdate<'a> {
    pub id: Uuid,
    /// 修改前的版本號（§9.4）。
    pub version: i32,
    /// 完整的資料（JSON 物件），所有實體欄位依此取代。
    pub data: &'a Value,
    /// 自訂欄位（JSON 物件），整個取代 `custom_data`。
    pub custom_data: &'a Value,
    pub formula_errors: Option<&'a Value>,
    pub updated_by: Uuid,
}

/// 取代資料的實體欄位、`custom_data`、`formula_errors` 並遞增 `version`。版本號不符時影響 0 筆。
pub async fn update_record<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    update: &SystemRecordUpdate<'_>,
) -> Result<PgQueryResult, DbError> {
    Ok(update_query(table, update)
        .build()
        .execute(executor)
        .await?)
}

fn update_query(table: &SystemTable, update: &SystemRecordUpdate<'_>) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new("UPDATE \"");
    qb.push(&table.name).push("\" SET ");
    for c in &table.columns {
        qb.push("\"").push(&c.name).push("\" = ");
        SystemTable::push_column_value(&mut qb, c);
        qb.push(", ");
    }
    qb.push("custom_data = ")
        .push_bind(update.custom_data)
        .push(", formula_errors = ")
        .push_bind(update.formula_errors)
        .push(", updated_by = ")
        .push_bind(update.updated_by)
        .push(", version = version + 1 FROM (SELECT ")
        .push_bind(update.data)
        .push("::jsonb AS d) AS v WHERE id = ")
        .push_bind(update.id)
        .push(" AND version = ")
        .push_bind(update.version)
        .push(" AND deleted_at IS NULL");
    qb
}

/// 軟刪除資料並遞增 `version`。版本號不符時影響 0 筆。
pub async fn soft_delete_record<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    id: Uuid,
    version: i32,
    deleted_at: OffsetDateTime,
    deleted_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let mut qb = QueryBuilder::<Postgres>::new("UPDATE \"");
    qb.push(&table.name)
        .push("\" SET deleted_at = ")
        .push_bind(deleted_at)
        .push(", updated_by = ")
        .push_bind(deleted_by)
        .push(", version = version + 1 WHERE id = ")
        .push_bind(id)
        .push(" AND version = ")
        .push_bind(version)
        .push(" AND deleted_at IS NULL");
    Ok(qb.build().execute(executor).await?)
}

/// 依條件分頁查詢資料。`filter.entity_id` 不使用（資料表只存一個資料類別）；條件與排序的格式、
/// 預設排序與 [`crate::record::list_records`] 相同，系統欄位比較實體欄位、其他欄位比較 `custom_data`。
pub async fn list_records<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    filter: &RecordListFilter,
    limit: i64,
    offset: i64,
) -> Result<Vec<RecordRow>, DbError> {
    let mut qb = list_query(table, filter);
    qb.push(" LIMIT ").push_bind(limit);
    qb.push(" OFFSET ").push_bind(offset);
    Ok(qb.build_query_as::<RecordRow>().fetch_all(executor).await?)
}

fn list_query(table: &SystemTable, filter: &RecordListFilter) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new("");
    table.push_select(&mut qb);
    qb.push(" WHERE deleted_at IS NULL");
    push_conditions(&mut qb, Source::Table(table), &filter.conditions);
    push_order(&mut qb, Source::Table(table), filter.order.as_ref());
    qb
}

/// 符合條件的資料筆數。條件與 [`list_records`] 相同。
pub async fn count_records<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    filter: &RecordListFilter,
) -> Result<i64, DbError> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM \"");
    qb.push(&table.name).push("\" WHERE deleted_at IS NULL");
    push_conditions(&mut qb, Source::Table(table), &filter.conditions);
    Ok(qb.build_query_scalar::<i64>().fetch_one(executor).await?)
}

/// 選擇器搜尋的一批資料：未刪除、依 `id` 由新到舊，`before` 有值時只取 id 小於它的資料。
/// 資料表有 [`IS_ACTIVE_COLUMN`] 系統欄位時，排除 `is_active = false` 的資料（§7.1）。
pub async fn list_records_before<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    before: Option<Uuid>,
    limit: i64,
) -> Result<Vec<RecordRow>, DbError> {
    let mut qb = before_query(table, before, limit);
    Ok(qb.build_query_as::<RecordRow>().fetch_all(executor).await?)
}

fn before_query(table: &SystemTable, before: Option<Uuid>, limit: i64) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new("");
    table.push_select(&mut qb);
    qb.push(" WHERE deleted_at IS NULL");
    if table
        .column(IS_ACTIVE_COLUMN)
        .is_some_and(|c| c.column_type == ColumnType::Boolean)
    {
        qb.push(" AND \"")
            .push(IS_ACTIVE_COLUMN)
            .push("\" IS NOT FALSE");
    }
    if let Some(before) = before {
        qb.push(" AND id < ").push_bind(before);
    }
    qb.push(" ORDER BY id DESC LIMIT ").push_bind(limit);
    qb
}

/// 是否有其他未刪除的資料在 `field_key` 有相同的值（`id` 為 `exclude_id` 的除外）。用於違反唯一索引時
/// 找出重複的欄位。
///
/// - 系統欄位：與 migration 建立的唯一索引相同，以 `lower(<欄位>)` 比較（不分大小寫，§7.2）
/// - 自訂欄位：比較 `custom_data -> field_key`（與動態唯一索引相同，§7.5）
pub async fn value_taken<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    exclude_id: Uuid,
    field_key: &str,
    value: &Value,
) -> Result<bool, DbError> {
    let mut qb = value_taken_query(table, exclude_id, field_key, value);
    Ok(qb.build_query_scalar::<bool>().fetch_one(executor).await?)
}

fn value_taken_query(
    table: &SystemTable,
    exclude_id: Uuid,
    field_key: &str,
    value: &Value,
) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new("SELECT EXISTS (SELECT 1 FROM \"");
    qb.push(&table.name)
        .push("\" WHERE id <> ")
        .push_bind(exclude_id)
        .push(" AND deleted_at IS NULL AND ");
    match table.column(field_key) {
        Some(c) => {
            qb.push("lower(\"")
                .push(&c.name)
                .push("\"::text) = lower(")
                .push_bind(value)
                .push("::jsonb #>> '{}')");
        }
        None => {
            qb.push("custom_data -> ")
                .push_bind(field_key.to_owned())
                .push(" = ")
                .push_bind(value);
        }
    }
    qb.push(")");
    qb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{CompareOp, JsonCast, OrderTarget, RecordCondition, RecordOrder};

    fn table() -> SystemTable {
        SystemTable::new(
            Uuid::nil(),
            "customers",
            vec![
                SystemColumn {
                    name: "code".into(),
                    column_type: ColumnType::Text,
                },
                SystemColumn {
                    name: "credit_limit".into(),
                    column_type: ColumnType::Numeric,
                },
                SystemColumn {
                    name: "is_active".into(),
                    column_type: ColumnType::Boolean,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn rejects_unsafe_names() {
        assert!(matches!(
            SystemTable::new(Uuid::nil(), "units\"; DROP TABLE users; --", vec![]),
            Err(DbError::UnsafeIdentifier(_))
        ));
        assert!(matches!(
            SystemTable::new(
                Uuid::nil(),
                "units",
                vec![SystemColumn {
                    name: "code') --".into(),
                    column_type: ColumnType::Text,
                }],
            ),
            Err(DbError::UnsafeIdentifier(s)) if s == "code') --"
        ));
    }

    #[test]
    fn select_merges_columns_into_data() {
        let qb = find_query(&table(), Uuid::nil(), true);
        assert_eq!(
            qb.sql().as_str(),
            "SELECT id, $1::uuid AS entity_id, NULL::text AS record_no, \
             (custom_data || jsonb_strip_nulls(jsonb_build_object('code', \"code\", \
             'credit_limit', \"credit_limit\"::text, 'is_active', \"is_active\"))) AS data, \
             formula_errors, created_at, created_by, updated_at, updated_by, version \
             FROM \"customers\" WHERE id = $2 AND deleted_at IS NULL FOR UPDATE"
        );
    }

    #[test]
    fn insert_and_update_read_columns_from_data() {
        let data = serde_json::json!({});
        let new = NewSystemRecord {
            id: Uuid::nil(),
            data: &data,
            custom_data: &data,
            formula_errors: None,
            created_by: Uuid::nil(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert_eq!(
            insert_query(&table(), &new).sql().as_str(),
            "INSERT INTO \"customers\" (id, \"code\", \"credit_limit\", \"is_active\", custom_data, \
             formula_errors, created_at, created_by, updated_at, updated_by) SELECT $1, \
             (v.d ->> 'code'), (v.d ->> 'credit_limit')::numeric, (v.d ->> 'is_active')::boolean, \
             $2, $3, $4, $5, $6, $7 FROM (SELECT $8::jsonb AS d) AS v"
        );
        let update = SystemRecordUpdate {
            id: Uuid::nil(),
            version: 1,
            data: &data,
            custom_data: &data,
            formula_errors: None,
            updated_by: Uuid::nil(),
        };
        assert_eq!(
            update_query(&table(), &update).sql().as_str(),
            "UPDATE \"customers\" SET \"code\" = (v.d ->> 'code'), \
             \"credit_limit\" = (v.d ->> 'credit_limit')::numeric, \
             \"is_active\" = (v.d ->> 'is_active')::boolean, custom_data = $1, \
             formula_errors = $2, updated_by = $3, version = version + 1 \
             FROM (SELECT $4::jsonb AS d) AS v WHERE id = $5 AND version = $6 \
             AND deleted_at IS NULL"
        );
    }

    #[test]
    fn list_compares_columns_and_custom_data() {
        let filter = RecordListFilter {
            entity_id: Uuid::nil(),
            conditions: vec![
                RecordCondition::Compare {
                    field_key: "credit_limit".into(),
                    cast: JsonCast::Numeric,
                    op: CompareOp::Gte,
                    value: "1".into(),
                },
                RecordCondition::Contains {
                    field_key: "code".into(),
                    text: "a".into(),
                },
                RecordCondition::HasValue {
                    field_key: "code".into(),
                    has_value: false,
                },
                RecordCondition::Compare {
                    field_key: "region".into(),
                    cast: JsonCast::Text,
                    op: CompareOp::Eq,
                    value: "北區".into(),
                },
                RecordCondition::HasValue {
                    field_key: "region".into(),
                    has_value: true,
                },
                RecordCondition::RecordNoEq("X".into()),
            ],
            order: Some(RecordOrder {
                target: OrderTarget::Field {
                    field_key: "code".into(),
                    cast: JsonCast::Text,
                },
                descending: false,
            }),
        };
        let sql = list_query(&table(), &filter).sql().as_str().to_owned();
        let where_clause = &sql[sql.find(" WHERE").unwrap()..];
        assert_eq!(
            where_clause,
            " WHERE deleted_at IS NULL \
             AND (\"credit_limit\"::numeric) >= $2::numeric \
             AND (\"code\"::text) ILIKE $3 ESCAPE '\\' \
             AND (\"code\" IS NULL) \
             AND (custom_data ->> $4) = $5 \
             AND (custom_data ? $6) \
             AND NULL::text = $7 \
             ORDER BY (\"code\"::text) ASC NULLS LAST, id ASC"
        );
    }

    #[test]
    fn lookup_excludes_inactive_when_column_exists() {
        let sql = before_query(&table(), Some(Uuid::nil()), 10)
            .sql()
            .as_str()
            .to_owned();
        assert!(
            sql.ends_with(
                " WHERE deleted_at IS NULL AND \"is_active\" IS NOT FALSE AND id < $2 \
                 ORDER BY id DESC LIMIT $3"
            ),
            "{sql}"
        );
        let units = SystemTable::new(Uuid::nil(), "units", vec![]).unwrap();
        let sql = before_query(&units, None, 10).sql().as_str().to_owned();
        assert!(
            sql.ends_with(" WHERE deleted_at IS NULL ORDER BY id DESC LIMIT $2"),
            "{sql}"
        );
    }

    #[test]
    fn value_taken_is_case_insensitive_for_columns() {
        let value = serde_json::json!("A001");
        let sql = value_taken_query(&table(), Uuid::nil(), "code", &value)
            .sql()
            .as_str()
            .to_owned();
        assert_eq!(
            sql,
            "SELECT EXISTS (SELECT 1 FROM \"customers\" WHERE id <> $1 AND deleted_at IS NULL \
             AND lower(\"code\"::text) = lower($2::jsonb #>> '{}'))"
        );
        let sql = value_taken_query(&table(), Uuid::nil(), "region", &value)
            .sql()
            .as_str()
            .to_owned();
        assert!(sql.ends_with("AND custom_data -> $2 = $3)"), "{sql}");
    }
}
