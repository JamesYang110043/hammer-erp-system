//! 銷貨用商品選擇 API（docs/architecture.md §8.9「銷貨用商品選擇 API」，M11）。
//!
//! 訂單、出貨單的明細選商品時使用：除了商品標題（與通用 `lookup` 相同），也比對這位客戶的客戶品名，
//! 並回傳依帶入順序決定的品名。格式見 [`hammer_shared::sales`]。
//!
//! 商品標題不存入資料庫，所以與通用 `lookup`（[`crate::records::lookup_records`]）相同，依 id 由新到舊
//! 分批讀取未刪除、啟用中的商品，產生標題後比對。客戶品名符合的排在前面，所以要讀完所有商品（除非
//! 第一組已滿）才能確定順序。

use std::collections::HashMap;

use hammer_db::DbError;
use hammer_db::sales as db;
use hammer_db::sqlx::PgPool;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::permission;
use hammer_shared::record::{LookupQuery, QueryParamError, title_matches};
use hammer_shared::sales::{SalesProductLookupItem, product_lookup_param};
use serde_json::{Map, Value};

use crate::CoreError;
use crate::auth::AuthContext;
use crate::master_data::PRODUCT;
use crate::record_titles::Titles;
use crate::records::load_entity;

/// 客戶不存在或已刪除。
pub const MSG_CUSTOMER_NOT_FOUND: &str = "找不到此客戶";

/// 每批讀取的商品筆數。
const BATCH: i64 = 200;

/// `GET /api/v1/sales/product-lookup`。需要 `record.read`。
///
/// - `customer_id` 不是 uuid → [`CoreError::InvalidParam`]；客戶不存在或已刪除 → [`CoreError::NotFound`]
/// - `limit` 不是整數 → [`CoreError::BadRequest`]；小於 1 → [`CoreError::InvalidParam`]
pub async fn product_lookup(
    pool: &PgPool,
    ctx: &AuthContext,
    pairs: &[(String, String)],
) -> Result<Vec<SalesProductLookupItem>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let query = LookupQuery::from_pairs(pairs).map_err(|err| match err {
        QueryParamError::NotInteger { .. } => CoreError::BadRequest(err.message()),
        QueryParamError::Invalid { param, message } => CoreError::InvalidParam { param, message },
    })?;
    let customer_id = customer_param(pairs)?;
    let limit = query.effective_limit() as usize;
    let terms = query.terms();

    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let customer_names: HashMap<Id, String> = match customer_id {
        Some(customer) => {
            if !db::customer_exists(&mut *conn, customer).await? {
                return Err(CoreError::NotFound(MSG_CUSTOMER_NOT_FOUND.into()));
            }
            db::all_customer_product_names(&mut *conn, customer)
                .await?
                .into_iter()
                .collect()
        }
        None => HashMap::new(),
    };

    let (entity, fields, store) = load_entity(&mut conn, PRODUCT).await?;
    let titles = Titles::load(&mut conn, &entity, &fields).await?;
    // 第一組：每個詞都出現在客戶品名中；第二組：其他符合的商品
    let mut first = Vec::new();
    let mut second = Vec::new();
    let mut before = None;
    loop {
        let rows = store
            .list_before(&mut conn, entity.id, before, BATCH)
            .await?;
        let exhausted = (rows.len() as i64) < BATCH;
        before = rows.last().map(|r| r.id);
        let related = titles
            .related(&mut conn, rows.iter().filter_map(|r| r.data.as_object()))
            .await?;
        for row in rows {
            let Some(data) = row.data.as_object() else {
                continue;
            };
            let title = titles.render(row.id, row.record_no.as_deref(), data, &related);
            let customer_name = customer_names.get(&row.id);
            match group(&title, customer_name.map(String::as_str), &terms) {
                Some(Group::CustomerName) if first.len() < limit => {
                    first.push(item(row.id, title, data, customer_name.cloned()));
                }
                Some(Group::Other) if second.len() < limit => {
                    second.push(item(row.id, title, data, customer_name.cloned()));
                }
                _ => {}
            }
        }
        if exhausted || first.len() >= limit {
            break;
        }
    }
    first.extend(second);
    first.truncate(limit);
    Ok(first)
}

/// `customer_id` 參數：空字串視為未指定，重複時以最後一個為準。
fn customer_param(pairs: &[(String, String)]) -> Result<Option<Id>, CoreError> {
    let Some((_, raw)) = pairs
        .iter()
        .rev()
        .find(|(name, _)| name == product_lookup_param::CUSTOMER_ID)
    else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    Id::try_parse(raw)
        .map(Some)
        .map_err(|_| CoreError::InvalidParam {
            param: product_lookup_param::CUSTOMER_ID.to_owned(),
            message: "customer_id 必須是 uuid".to_owned(),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// 每個詞都出現在客戶品名中（`terms` 為空時：有客戶品名）。
    CustomerName,
    /// 其他符合的商品。
    Other,
}

/// 比對：每個詞都出現在「標題 + 客戶品名」中；不符合時為 `None`。
fn group(title: &str, customer_name: Option<&str>, terms: &[String]) -> Option<Group> {
    if let Some(name) = customer_name
        && title_matches(name, terms)
    {
        return Some(Group::CustomerName);
    }
    let text = match customer_name {
        Some(name) => format!("{title} {name}"),
        None => title.to_owned(),
    };
    title_matches(&text, terms).then_some(Group::Other)
}

fn item(
    id: Id,
    title: String,
    data: &Map<String, Value>,
    customer_product_name: Option<String>,
) -> SalesProductLookupItem {
    let text = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_owned);
    let name = text("name").unwrap_or_default();
    SalesProductLookupItem {
        id,
        title,
        code: text("code").unwrap_or_default(),
        spec: text("spec"),
        product_type: text("product_type").unwrap_or_default(),
        default_sale_price: data
            .get("default_sale_price")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<Decimal>().ok()),
        product_name: customer_product_name
            .clone()
            .unwrap_or_else(|| name.clone()),
        customer_product_name,
        name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(q: &str) -> Vec<String> {
        q.split_whitespace().map(str::to_lowercase).collect()
    }

    #[test]
    fn customer_name_matches_come_first() {
        let title = "P001 扳手 10mm";
        // 客戶品名包含所有詞 → 第一組
        assert_eq!(
            group(title, Some("X 型扳手"), &terms("扳手")),
            Some(Group::CustomerName)
        );
        assert_eq!(
            group(title, Some("工具組零件 03"), &terms("零件")),
            Some(Group::CustomerName)
        );
        // 只有標題符合 → 第二組
        assert_eq!(
            group(title, Some("工具組零件 03"), &terms("p001")),
            Some(Group::Other)
        );
        // 料號與客戶品名混用 → 第二組
        assert_eq!(
            group(title, Some("X 型扳手"), &terms("P001 x 型")),
            Some(Group::Other)
        );
        // 沒有客戶品名
        assert_eq!(group(title, None, &terms("扳手")), Some(Group::Other));
        assert_eq!(group(title, None, &terms("零件")), None);
        // 沒有關鍵字：有客戶品名的在前
        assert_eq!(group(title, Some("X"), &[]), Some(Group::CustomerName));
        assert_eq!(group(title, None, &[]), Some(Group::Other));
    }

    #[test]
    fn customer_param_parses_uuid_or_empty() {
        let id = "01a211f5-20e9-7b08-9d17-2a0dbce98c12";
        let pairs = |v: &str| vec![("customer_id".to_owned(), v.to_owned())];
        assert_eq!(customer_param(&[]).unwrap(), None);
        assert_eq!(customer_param(&pairs(" ")).unwrap(), None);
        assert_eq!(
            customer_param(&pairs(id)).unwrap(),
            Some(Id::try_parse(id).unwrap())
        );
        assert!(matches!(
            customer_param(&pairs("x")),
            Err(CoreError::InvalidParam { ref param, .. }) if param == "customer_id"
        ));
    }

    #[test]
    fn item_uses_customer_name_as_product_name() {
        let data = serde_json::json!({ "code": "P001", "name": "扳手 10mm", "spec": "10mm",
                                       "product_type": "goods",
                                       "default_sale_price": "120.0000" });
        let data = data.as_object().unwrap();
        let a = item(
            Id::nil(),
            "P001 扳手 10mm".into(),
            data,
            Some("X 型扳手".into()),
        );
        assert_eq!(a.product_name, "X 型扳手");
        assert_eq!(a.customer_product_name.as_deref(), Some("X 型扳手"));
        assert_eq!(a.default_sale_price, Some("120.0000".parse().unwrap()));
        let b = item(Id::nil(), "P001 扳手 10mm".into(), data, None);
        assert_eq!(b.product_name, "扳手 10mm");
        assert_eq!(b.spec.as_deref(), Some("10mm"));
    }
}
