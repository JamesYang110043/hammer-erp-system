//! 明細品名的帶入順序（docs/architecture.md §8.9「明細的品名」）與批次取得明細品名 API
//! （§8.9「批次取得明細品名 API」，M12）。
//!
//! 帶入順序：這位客戶未刪除的客戶品名 → 商品的 `name`。[`resolve`] 由存檔時的帶入
//! （[`super::save`]）與 [`product_names`] 共用，一次查詢取得所有商品。

use std::collections::{HashMap, HashSet};

use hammer_db::DbError;
use hammer_db::sales as db;
use hammer_db::sqlx::{PgConnection, PgPool};
use hammer_shared::Id;
use hammer_shared::permission;
use hammer_shared::sales::SalesProductName;
use hammer_shared::sales::product_names_param::{CUSTOMER_ID, MAX_PRODUCTS, PRODUCT_ID};

use super::product_lookup::MSG_CUSTOMER_NOT_FOUND;
use crate::CoreError;
use crate::auth::AuthContext;

/// 一個商品依帶入順序決定的品名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    /// 這位客戶的客戶品名。
    pub customer_product_name: Option<String>,
    /// `customer_product_name` ?? 商品的 `name`。
    pub product_name: String,
}

/// 依帶入順序決定 `products` 的品名（一次查詢）。未刪除的商品（含停用）才有結果；`customer_id` 為
/// `None` 時一律為商品的 `name`。
pub(crate) async fn resolve(
    conn: &mut PgConnection,
    customer_id: Option<Id>,
    products: &[Id],
) -> Result<HashMap<Id, ResolvedName>, CoreError> {
    let rows = db::product_name_sources(&mut *conn, customer_id, products).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.product_id,
                ResolvedName {
                    product_name: r.customer_product_name.clone().unwrap_or(r.name),
                    customer_product_name: r.customer_product_name,
                },
            )
        })
        .collect())
}

/// `GET /api/v1/sales/product-names?customer_id=&product_id=…`。需要 `record.read`。
///
/// 參數錯誤 → [`CoreError::InvalidParam`]；客戶不存在或已刪除 → [`CoreError::NotFound`]。
pub async fn product_names(
    pool: &PgPool,
    ctx: &AuthContext,
    pairs: &[(String, String)],
) -> Result<Vec<SalesProductName>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let (customer_id, products) = parse_params(pairs)?;

    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    if !db::customer_exists(&mut *conn, customer_id).await? {
        return Err(CoreError::NotFound(MSG_CUSTOMER_NOT_FOUND.into()));
    }
    let mut names = resolve(&mut conn, Some(customer_id), &products).await?;
    Ok(products
        .into_iter()
        .filter_map(|id| {
            names.remove(&id).map(|n| SalesProductName {
                product_id: id,
                customer_product_name: n.customer_product_name,
                product_name: n.product_name,
            })
        })
        .collect())
}

fn invalid(param: &str, message: &str) -> CoreError {
    CoreError::InvalidParam {
        param: param.to_owned(),
        message: message.to_owned(),
    }
}

/// 解析參數：`customer_id`（重複時以最後一個為準）與去除重複、保留第一次出現順序的 `product_id`。
fn parse_params(pairs: &[(String, String)]) -> Result<(Id, Vec<Id>), CoreError> {
    let customer = pairs
        .iter()
        .rev()
        .find(|(name, _)| name == CUSTOMER_ID)
        .map(|(_, v)| v.trim())
        .unwrap_or_default();
    if customer.is_empty() {
        return Err(invalid(CUSTOMER_ID, "請指定客戶"));
    }
    let customer =
        Id::try_parse(customer).map_err(|_| invalid(CUSTOMER_ID, "customer_id 必須是 uuid"))?;

    let raw: Vec<&str> = pairs
        .iter()
        .filter(|(name, _)| name == PRODUCT_ID)
        .map(|(_, v)| v.trim())
        .collect();
    if raw.is_empty() {
        return Err(invalid(PRODUCT_ID, "請指定商品"));
    }
    if raw.len() > MAX_PRODUCTS {
        return Err(invalid(
            PRODUCT_ID,
            &format!("一次最多 {MAX_PRODUCTS} 個商品"),
        ));
    }
    let mut seen = HashSet::new();
    let mut products = Vec::new();
    for value in raw {
        let id = Id::try_parse(value).map_err(|_| invalid(PRODUCT_ID, "product_id 必須是 uuid"))?;
        if seen.insert(id) {
            products.push(id);
        }
    }
    Ok((customer, products))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "01a211f5-20e9-7b08-9d17-2a0dbce98c12";
    const P1: &str = "01a211f5-20e9-7b08-9d17-2a0dbce98c13";
    const P2: &str = "01a211f5-20e9-7b08-9d17-2a0dbce98c14";

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn param_of(err: CoreError) -> String {
        match err {
            CoreError::InvalidParam { param, .. } => param,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_and_dedups_products_in_order() {
        let (customer, products) = parse_params(&pairs(&[
            ("customer_id", C),
            ("product_id", P2),
            ("product_id", P1),
            ("product_id", P2),
        ]))
        .unwrap();
        assert_eq!(customer, Id::try_parse(C).unwrap());
        assert_eq!(
            products,
            [Id::try_parse(P2).unwrap(), Id::try_parse(P1).unwrap()]
        );
    }

    #[test]
    fn customer_is_required_uuid() {
        for p in [
            pairs(&[("product_id", P1)]),
            pairs(&[("customer_id", " "), ("product_id", P1)]),
            pairs(&[("customer_id", "x"), ("product_id", P1)]),
        ] {
            assert_eq!(param_of(parse_params(&p).unwrap_err()), "customer_id");
        }
    }

    #[test]
    fn products_count_and_format() {
        assert_eq!(
            param_of(parse_params(&pairs(&[("customer_id", C)])).unwrap_err()),
            "product_id"
        );
        assert_eq!(
            param_of(parse_params(&pairs(&[("customer_id", C), ("product_id", "x")])).unwrap_err()),
            "product_id"
        );
        // 上限以參數個數計（含重複）
        let mut many = vec![("customer_id", C)];
        many.extend(std::iter::repeat_n(("product_id", P1), MAX_PRODUCTS));
        assert!(parse_params(&pairs(&many)).is_ok());
        many.push(("product_id", P1));
        assert_eq!(
            param_of(parse_params(&pairs(&many)).unwrap_err()),
            "product_id"
        );
    }
}
