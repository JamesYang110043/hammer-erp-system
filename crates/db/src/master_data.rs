//! 基本資料的業務規則需要的查詢（docs/architecture.md §7.2、§8.6、§8.7）。
//!
//! - [`currency_is_active`]：廠商的 `default_currency_code` 是否為有效的幣別
//! - [`category_ancestors`]：商品分類往上的所有上層分類（檢查循環）
//! - [`count_products_by_unit`]、[`count_products_by_category`]、[`count_child_categories`]：刪除單位、
//!   分類前檢查是否還在使用中
//! - [`count_product_transactions`]：商品的庫存異動筆數（有異動的商品不可刪除、改類型或單位）
//!
//! 規則本身在 `hammer_core::master_data`。

use sqlx::PgExecutor;
use uuid::Uuid;

use crate::DbError;

/// `code` 是否為存在且啟用中的幣別。
pub async fn currency_is_active<'e>(
    executor: impl PgExecutor<'e>,
    code: &str,
) -> Result<bool, DbError> {
    let active = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM currencies WHERE code = $1 AND is_active) AS "active!""#,
        code,
    )
    .fetch_one(executor)
    .await?;
    Ok(active)
}

/// 商品分類 `id` 本身與往上的所有上層分類 id（含已刪除的分類）。`id` 不存在時為空。
///
/// 以 `UNION` 去除重複，資料庫中已有循環時也會結束。
pub async fn category_ancestors<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let rows = sqlx::query_scalar!(
        r#"
        WITH RECURSIVE chain (id, parent_id) AS (
            SELECT id, parent_id FROM product_categories WHERE id = $1
            UNION
            SELECT c.id, c.parent_id
            FROM product_categories c JOIN chain ON c.id = chain.parent_id
        )
        SELECT id AS "id!" FROM chain
        "#,
        id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 使用單位 `unit_id` 的未刪除商品數。
pub async fn count_products_by_unit<'e>(
    executor: impl PgExecutor<'e>,
    unit_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM products WHERE unit_id = $1 AND deleted_at IS NULL"#,
        unit_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 使用分類 `category_id` 的未刪除商品數。
pub async fn count_products_by_category<'e>(
    executor: impl PgExecutor<'e>,
    category_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM products WHERE category_id = $1 AND deleted_at IS NULL"#,
        category_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 上層分類為 `parent_id` 的未刪除分類數。
pub async fn count_child_categories<'e>(
    executor: impl PgExecutor<'e>,
    parent_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!" FROM product_categories
        WHERE parent_id = $1 AND deleted_at IS NULL
        "#,
        parent_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 商品 `product_id` 的庫存異動筆數（不論來源單據之後是否作廢，§7.2「商品被庫存引用時的限制」）。
pub async fn count_product_transactions<'e>(
    executor: impl PgExecutor<'e>,
    product_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM inventory_transactions WHERE product_id = $1"#,
        product_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}
