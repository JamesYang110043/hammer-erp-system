//! 自訂欄位的動態索引（`docs/architecture.md` §7.5），建立在 `custom_records` 上。
//!
//! - 可搜尋：`CREATE INDEX … ((data ->> 'field_key')) WHERE entity_id = '…' AND deleted_at IS NULL`
//! - 不可重複：`CREATE UNIQUE INDEX … (entity_id, (data ->> 'field_key'))
//!   WHERE entity_id = '…' AND deleted_at IS NULL`
//!
//! 兩者都以部分索引限定在單一資料類別，所以不同資料類別即使有相同的 `field_key`，
//! 索引與唯一性也互不影響。
//!
//! DDL 不能使用 bind 參數，因此索引名稱與 `field_key` 必須拼接進 SQL：
//! 拼接前一律檢查只含 `a-z`、`0-9`、`_`（[`is_safe_identifier`]），並加上引號
//! （名稱用 `"…"`，`field_key` 用 `'…'`）。`entity_id` 是 [`Uuid`]，格式固定。
//! 索引名稱的命名規則在 `hammer_core::schema::index`。
//!
//! 索引不使用 `CONCURRENTLY`，可以和欄位定義的修改放在同一個 transaction 內。

use serde_json::Value;
use sqlx::{AssertSqlSafe, PgExecutor};
use uuid::Uuid;

use crate::DbError;

/// PostgreSQL 識別字的長度上限（bytes）。
pub const MAX_IDENTIFIER_LEN: usize = 63;

/// 要建立的運算式索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpressionIndex<'a> {
    /// 索引名稱，必須符合 [`is_safe_identifier`]。
    pub name: &'a str,
    pub entity_id: Uuid,
    /// 必須符合 [`is_safe_identifier`]。
    pub field_key: &'a str,
    /// 部分唯一索引（不可重複）；否則為一般索引（可搜尋）。
    pub unique: bool,
}

/// 是否可以直接拼接進 DDL：1 到 [`MAX_IDENTIFIER_LEN`] 個字元，只含 `a-z`、`0-9`、`_`，
/// 且以英文字母開頭。
pub fn is_safe_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_IDENTIFIER_LEN
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn ensure_safe(s: &str) -> Result<(), DbError> {
    if is_safe_identifier(s) {
        Ok(())
    } else {
        Err(DbError::UnsafeIdentifier(s.to_owned()))
    }
}

/// 產生 `CREATE [UNIQUE] INDEX IF NOT EXISTS` 的 SQL。名稱或 `field_key` 不安全時回傳
/// [`DbError::UnsafeIdentifier`]。
pub fn create_index_sql(index: &ExpressionIndex<'_>) -> Result<String, DbError> {
    ensure_safe(index.name)?;
    ensure_safe(index.field_key)?;
    let (unique, columns) = if index.unique {
        (
            "UNIQUE ",
            format!("entity_id, (data ->> '{}')", index.field_key),
        )
    } else {
        ("", format!("(data ->> '{}')", index.field_key))
    };
    Ok(format!(
        "CREATE {unique}INDEX IF NOT EXISTS \"{name}\" ON custom_records ({columns}) \
         WHERE entity_id = '{entity_id}' AND deleted_at IS NULL",
        name = index.name,
        entity_id = index.entity_id,
    ))
}

/// 產生 `DROP INDEX IF EXISTS` 的 SQL。名稱不安全時回傳 [`DbError::UnsafeIdentifier`]。
pub fn drop_index_sql(name: &str) -> Result<String, DbError> {
    ensure_safe(name)?;
    Ok(format!("DROP INDEX IF EXISTS \"{name}\""))
}

/// 建立索引；已存在同名的索引時不動作。
///
/// 建立唯一索引時如果有重複的值，回傳違反 UNIQUE 限制的 [`DbError::Sqlx`]，
/// 所以應先用 [`find_duplicate_values`] 檢查。
pub async fn create_index<'e>(
    executor: impl PgExecutor<'e>,
    index: &ExpressionIndex<'_>,
) -> Result<(), DbError> {
    let sql = create_index_sql(index)?;
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(executor).await?;
    Ok(())
}

/// 移除索引；不存在時不動作。
pub async fn drop_index<'e>(executor: impl PgExecutor<'e>, name: &str) -> Result<(), DbError> {
    let sql = drop_index_sql(name)?;
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(executor).await?;
    Ok(())
}

/// 重複的值與擁有這個值的資料。
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateValueRow {
    /// `data -> field_key`（JSON 原值）。
    pub value: Value,
    /// 擁有這個值的資料筆數。
    pub count: i64,
    /// 擁有這個值的資料 id，依 `id` 排序，最多 `limit` 筆。
    pub record_ids: Vec<Uuid>,
    /// 重複的值的總數（每一列相同）。
    pub total: i64,
}

/// 未刪除的資料中，`data ->> field_key` 相同的值（與唯一索引的比較方式相同）。
///
/// 回傳最多 `limit` 個值，依值排序；每個值最多列出 `limit` 筆資料 id。沒有值的資料不列入。
pub async fn find_duplicate_values<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    field_key: &str,
    limit: i32,
) -> Result<Vec<DuplicateValueRow>, DbError> {
    let rows = sqlx::query_as!(
        DuplicateValueRow,
        r#"
        SELECT
            (array_agg(data -> $2 ORDER BY id))[1] AS "value!",
            count(*) AS "count!",
            (array_agg(id ORDER BY id))[1:$3] AS "record_ids!",
            count(*) OVER () AS "total!"
        FROM custom_records
        WHERE entity_id = $1 AND deleted_at IS NULL AND data ->> $2 IS NOT NULL
        GROUP BY data ->> $2
        HAVING count(*) > 1
        ORDER BY data ->> $2
        LIMIT $3
        "#,
        entity_id,
        field_key,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTITY: Uuid = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001);

    #[test]
    fn safe_identifier_rules() {
        for s in [
            "a",
            "cfs_repair_order_title_1a2b3c4d",
            "x_1",
            &"a".repeat(63),
        ] {
            assert!(is_safe_identifier(s), "{s}");
        }
        for s in [
            "",
            "1a",
            "_a",
            "A",
            "a-b",
            "a b",
            "a'b",
            "a\"b",
            "a;b",
            "ａ",
            &"a".repeat(64),
        ] {
            assert!(!is_safe_identifier(s), "{s}");
        }
    }

    #[test]
    fn searchable_index_sql() {
        let sql = create_index_sql(&ExpressionIndex {
            name: "cfs_x",
            entity_id: ENTITY,
            field_key: "title",
            unique: false,
        })
        .unwrap();
        assert_eq!(
            sql,
            "CREATE INDEX IF NOT EXISTS \"cfs_x\" ON custom_records ((data ->> 'title')) \
             WHERE entity_id = '01930000-0000-7000-8000-000000000001' AND deleted_at IS NULL"
        );
    }

    #[test]
    fn unique_index_sql() {
        let sql = create_index_sql(&ExpressionIndex {
            name: "cfu_x",
            entity_id: ENTITY,
            field_key: "code",
            unique: true,
        })
        .unwrap();
        assert_eq!(
            sql,
            "CREATE UNIQUE INDEX IF NOT EXISTS \"cfu_x\" ON custom_records \
             (entity_id, (data ->> 'code')) \
             WHERE entity_id = '01930000-0000-7000-8000-000000000001' AND deleted_at IS NULL"
        );
    }

    #[test]
    fn rejects_unsafe_input() {
        let unsafe_key = "x') ; DROP TABLE users; --";
        let err = create_index_sql(&ExpressionIndex {
            name: "cfs_x",
            entity_id: ENTITY,
            field_key: unsafe_key,
            unique: false,
        })
        .unwrap_err();
        assert!(matches!(err, DbError::UnsafeIdentifier(s) if s == unsafe_key));

        let err = create_index_sql(&ExpressionIndex {
            name: "x\"; DROP TABLE users; --",
            entity_id: ENTITY,
            field_key: "title",
            unique: true,
        })
        .unwrap_err();
        assert!(matches!(err, DbError::UnsafeIdentifier(_)));

        assert!(matches!(
            drop_index_sql("a\"b"),
            Err(DbError::UnsafeIdentifier(_))
        ));
        assert_eq!(
            drop_index_sql("cfu_x").unwrap(),
            "DROP INDEX IF EXISTS \"cfu_x\""
        );
    }
}
