//! 商品的客戶品名（docs/architecture.md §8.7「商品畫面的客戶品名」，M12）。
//!
//! 通用關聯不支援反向列表（§7.3），所以以專屬端點列出一個商品在各客戶的客戶品名。第一版唯讀，
//! 維護仍在客戶畫面。客戶標題依客戶的 `title_template` 產生（與關聯的標題相同）。

use std::collections::HashMap;

use hammer_db::DbError;
use hammer_db::sales as db;
use hammer_db::sqlx::PgPool;
use hammer_shared::Id;
use hammer_shared::permission;
use hammer_shared::sales::ProductCustomerName;

use crate::CoreError;
use crate::auth::AuthContext;
use crate::master_data::CUSTOMER;
use crate::record_titles::Titles;
use crate::records::load_entity;

/// 商品不存在或已刪除。
pub const MSG_PRODUCT_NOT_FOUND: &str = "找不到此商品";

/// `GET /api/v1/products/{id}/customer-product-names`。需要 `record.read`。
///
/// 商品不存在或已刪除 → [`CoreError::NotFound`]。
pub async fn product_customer_names(
    pool: &PgPool,
    ctx: &AuthContext,
    product_id: Id,
) -> Result<Vec<ProductCustomerName>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    if !db::product_exists(&mut *conn, product_id).await? {
        return Err(CoreError::NotFound(MSG_PRODUCT_NOT_FOUND.into()));
    }
    let rows = db::customer_names_of_product(&mut *conn, product_id).await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // 客戶標題：讀取客戶資料後依客戶的標題範本產生
    let (entity, fields, store) = load_entity(&mut conn, CUSTOMER).await?;
    let titles = Titles::load(&mut conn, &entity, &fields).await?;
    let mut ids: Vec<Id> = rows.iter().map(|r| r.customer_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let customers = store.find_by_ids(&mut conn, entity.id, &ids).await?;
    let related = titles
        .related(
            &mut conn,
            customers.iter().filter_map(|r| r.data.as_object()),
        )
        .await?;
    let title_of: HashMap<Id, String> = customers
        .iter()
        .filter_map(|c| {
            let data = c.data.as_object()?;
            Some((
                c.id,
                titles.render(c.id, c.record_no.as_deref(), data, &related),
            ))
        })
        .collect();

    Ok(rows
        .into_iter()
        .map(|r| ProductCustomerName {
            id: r.id,
            customer_title: title_of.get(&r.customer_id).cloned().unwrap_or_default(),
            customer_id: r.customer_id,
            customer_is_active: r.customer_is_active,
            name: r.name,
            notes: r.notes,
        })
        .collect())
}
