//! 銷貨用商品選擇 API（docs/architecture.md §8.9「銷貨用商品選擇 API」，M11；效能 M12）。
//!
//! 訂單、出貨單的明細選商品時使用：除了商品標題（與通用 `lookup` 相同），也比對這位客戶的客戶品名，
//! 並回傳依帶入順序決定的品名。格式見 [`hammer_shared::sales`]。
//!
//! 做法（§8.9「效能」，結果與排序與規格相同）：
//!
//! - 第一組（每個詞都出現在客戶品名中）由 SQL 取得（[`db::lookup_customer_name_matches`]）
//! - 商品的 `title_template` 為預設 [`DEFAULT_TITLE_TEMPLATE`] 時，第二組也由 SQL 取得
//!   （[`db::lookup_default_title_matches`]，`code`、`name`、客戶品名任一個 `ILIKE`）。詞不含空白，
//!   所以「以空白連接的比對文字」中的符合不會跨欄位，結果與產生標題後比對相同
//! - 範本被修改過時，第二組依 id 由新到舊分批讀取商品、產生標題後比對（[`scan`]），湊滿筆數就停止
//!
//! 詞中的 `\`、`%`、`_` 先跳脫（[`like_pattern`]），視為一般字元。

use std::collections::{HashMap, HashSet};

use hammer_db::DbError;
use hammer_db::sales as db;
use hammer_db::sqlx::{PgConnection, PgPool};
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::permission;
use hammer_shared::record::{LookupQuery, QueryParamError, title_matches};
use hammer_shared::sales::{SalesProductLookupItem, product_lookup_param};
use hammer_shared::schema::{EntityDefinition, FieldDefinition};
use serde_json::{Map, Value};

use crate::CoreError;
use crate::auth::AuthContext;
use crate::master_data::PRODUCT;
use crate::record_titles::Titles;
use crate::records::{Store, load_entity};

/// 客戶不存在或已刪除。
pub const MSG_CUSTOMER_NOT_FOUND: &str = "找不到此客戶";

/// 商品標題的預設範本；是這個範本時第二組以 SQL 篩選。
pub const DEFAULT_TITLE_TEMPLATE: &str = "{code} {name}";

/// 依範本產生標題比對時，每批讀取的商品筆數。
const BATCH: i64 = 200;

/// 第二組的做法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    /// 依範本決定：預設範本用 SQL，否則逐批產生標題比對。
    Auto,
    /// 一律逐批產生標題比對（回歸測試用，與 [`Strategy::Auto`] 的結果必須相同）。
    Scan,
}

/// `GET /api/v1/sales/product-lookup`。需要 `record.read`。
///
/// - `customer_id` 不是 uuid → [`CoreError::InvalidParam`]；客戶不存在或已刪除 → [`CoreError::NotFound`]
/// - `limit` 不是整數 → [`CoreError::BadRequest`]；小於 1 → [`CoreError::InvalidParam`]
pub async fn product_lookup(
    pool: &PgPool,
    ctx: &AuthContext,
    pairs: &[(String, String)],
) -> Result<Vec<SalesProductLookupItem>, CoreError> {
    lookup(pool, ctx, pairs, Strategy::Auto).await
}

/// 同 [`product_lookup`]，但第二組一律逐批產生標題比對（M11 的做法）。只供回歸測試比對結果。
#[doc(hidden)]
pub async fn product_lookup_by_scan(
    pool: &PgPool,
    ctx: &AuthContext,
    pairs: &[(String, String)],
) -> Result<Vec<SalesProductLookupItem>, CoreError> {
    lookup(pool, ctx, pairs, Strategy::Scan).await
}

async fn lookup(
    pool: &PgPool,
    ctx: &AuthContext,
    pairs: &[(String, String)],
    strategy: Strategy,
) -> Result<Vec<SalesProductLookupItem>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let query = LookupQuery::from_pairs(pairs).map_err(|err| match err {
        QueryParamError::NotInteger { .. } => CoreError::BadRequest(err.message()),
        QueryParamError::Invalid { param, message } => CoreError::InvalidParam { param, message },
    })?;
    let customer_id = customer_param(pairs)?;
    let limit = query.effective_limit() as usize;
    let terms = query.terms();
    let patterns: Vec<String> = terms.iter().map(|t| like_pattern(t)).collect();

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

    // 第一組：每個詞都出現在客戶品名中
    let first_ids = match customer_id {
        Some(customer) => {
            db::lookup_customer_name_matches(&mut *conn, customer, &patterns, limit as i64).await?
        }
        None => Vec::new(),
    };
    let rest = limit - first_ids.len();
    let mut items = load_items(
        &mut conn,
        &entity,
        &store,
        &titles,
        &first_ids,
        &customer_names,
    )
    .await?;
    if rest == 0 {
        return Ok(items);
    }

    // 第二組：其他符合的商品
    if strategy == Strategy::Auto && default_title(&entity, &fields) {
        let ids = db::lookup_default_title_matches(&mut *conn, customer_id, &patterns, rest as i64)
            .await?;
        items.extend(load_items(&mut conn, &entity, &store, &titles, &ids, &customer_names).await?);
    } else {
        let first: HashSet<Id> = first_ids.into_iter().collect();
        items.extend(
            scan(
                &mut conn,
                &entity,
                &store,
                &titles,
                &customer_names,
                &terms,
                &first,
                rest,
            )
            .await?,
        );
    }
    Ok(items)
}

/// 商品標題是否為預設範本，且 `code`、`name` 都是有效的欄位（停用的欄位在標題中沒有值）。
fn default_title(entity: &EntityDefinition, fields: &[FieldDefinition]) -> bool {
    entity.title_template.as_deref().map(str::trim) == Some(DEFAULT_TITLE_TEMPLATE)
        && ["code", "name"]
            .iter()
            .all(|key| fields.iter().any(|f| f.field_key == *key && f.is_active))
}

/// `ILIKE` 的樣式：跳脫 `\`、`%`、`_` 後前後加上 `%`（包含）。
fn like_pattern(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 2);
    out.push('%');
    for c in term.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// 依 `ids` 的順序讀取商品並轉成結果（已刪除的略過）。
async fn load_items(
    conn: &mut PgConnection,
    entity: &EntityDefinition,
    store: &Store,
    titles: &Titles<'_>,
    ids: &[Id],
    customer_names: &HashMap<Id, String>,
) -> Result<Vec<SalesProductLookupItem>, CoreError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = store.find_by_ids(&mut *conn, entity.id, ids).await?;
    let related = titles
        .related(&mut *conn, rows.iter().filter_map(|r| r.data.as_object()))
        .await?;
    let mut by_id: HashMap<Id, SalesProductLookupItem> = HashMap::new();
    for row in &rows {
        let Some(data) = row.data.as_object().filter(|_| !row.deleted) else {
            continue;
        };
        let title = titles.render(row.id, row.record_no.as_deref(), data, &related);
        by_id.insert(
            row.id,
            item(row.id, title, data, customer_names.get(&row.id).cloned()),
        );
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// 第二組（範本被修改過時）：依 id 由新到舊分批讀取未刪除、啟用中的商品，產生標題後比對；
/// 略過第一組的商品，湊滿 `limit` 筆就停止。
#[allow(clippy::too_many_arguments)]
async fn scan(
    conn: &mut PgConnection,
    entity: &EntityDefinition,
    store: &Store,
    titles: &Titles<'_>,
    customer_names: &HashMap<Id, String>,
    terms: &[String],
    first: &HashSet<Id>,
    limit: usize,
) -> Result<Vec<SalesProductLookupItem>, CoreError> {
    let mut out = Vec::new();
    let mut before = None;
    loop {
        let rows = store
            .list_before(&mut *conn, entity.id, before, BATCH)
            .await?;
        let exhausted = (rows.len() as i64) < BATCH;
        before = rows.last().map(|r| r.id);
        let related = titles
            .related(&mut *conn, rows.iter().filter_map(|r| r.data.as_object()))
            .await?;
        for row in rows {
            if first.contains(&row.id) {
                continue;
            }
            let Some(data) = row.data.as_object() else {
                continue;
            };
            let title = titles.render(row.id, row.record_no.as_deref(), data, &related);
            let customer_name = customer_names.get(&row.id);
            if group(&title, customer_name.map(String::as_str), terms).is_some() {
                out.push(item(row.id, title, data, customer_name.cloned()));
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
        if exhausted {
            return Ok(out);
        }
    }
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
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("扳手"), "%扳手%");
        assert_eq!(like_pattern("10%"), "%10\\%%");
        assert_eq!(like_pattern("a_b"), "%a\\_b%");
        assert_eq!(like_pattern("c:\\x"), "%c:\\\\x%");
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
