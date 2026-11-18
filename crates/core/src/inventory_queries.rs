//! 庫存查詢（docs/architecture.md §8.10「庫存查詢 API」、§10 分頁）。型別在 [`hammer_shared::inventory`]。
//!
//! - [`list_stock_balances`]：商品庫存（數量、台幣庫存金額、低於安全庫存）
//! - [`list_product_lots`]：某商品的批號（先進先出順序）
//! - [`list_transactions`]：異動流水帳（新到舊）
//!
//! 都需要 `record.read`。篩選參數格式錯誤 → [`CoreError::InvalidField`]（`422 VALIDATION`，
//! `details.field` 為參數名稱）；空字串的參數視為未指定。

use std::str::FromStr;

use hammer_db::inventory::{
    self as db, LotListFilter, LotListRow, StockBalanceFilter, StockBalanceRow, TransactionFilter,
    TransactionListRow,
};
use hammer_db::sqlx::PgPool;
use hammer_shared::decimal::{Decimal, RoundingStrategy};
use hammer_shared::inventory::{
    InventoryLot, InventoryTransaction, LotQuery, StockBalance, StockBalanceQuery,
    TransactionQuery, TxnType, query_field,
};
use hammer_shared::pagination::{Page, PageRequest};
use hammer_shared::{Id, permission};
use time::Date;
use time::macros::format_description;

use crate::CoreError;
use crate::auth::AuthContext;
use crate::inventory::{GOODS, qty4};

/// 商品不存在或已刪除時的錯誤訊息。
pub const PRODUCT_NOT_FOUND: &str = "找不到此商品";

/// 金額的小數位數（`NUMERIC(18,2)`）。
const MONEY_SCALE: u32 = 2;

/// 四捨五入到 2 位小數並補齊（台幣金額的格式）。
pub fn money2(value: Decimal) -> Decimal {
    let mut v = value.round_dp_with_strategy(MONEY_SCALE, RoundingStrategy::MidpointAwayFromZero);
    v.rescale(MONEY_SCALE);
    v
}

fn invalid(field: &'static str, message: impl Into<String>) -> CoreError {
    CoreError::InvalidField {
        field,
        message: message.into(),
    }
}

/// 去除前後空白；空字串視為未指定。
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn parse_bool(field: &'static str, value: Option<&str>) -> Result<Option<bool>, CoreError> {
    match non_empty(value) {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(_) => Err(invalid(field, "必須是 true 或 false")),
    }
}

fn parse_id(field: &'static str, value: Option<&str>) -> Result<Option<Id>, CoreError> {
    non_empty(value)
        .map(|s| Id::from_str(s).map_err(|_| invalid(field, "不是有效的 id")))
        .transpose()
}

fn parse_date(field: &'static str, value: Option<&str>) -> Result<Option<Date>, CoreError> {
    non_empty(value)
        .map(|s| {
            Date::parse(s, format_description!("[year]-[month]-[day]"))
                .map_err(|_| invalid(field, "日期格式必須是 YYYY-MM-DD"))
        })
        .transpose()
}

fn page_of(page: Option<u32>, page_size: Option<u32>) -> Result<PageRequest, CoreError> {
    PageRequest::new(page, page_size).map_err(|err| invalid(err.field, err.message()))
}

fn total_of(n: i64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 商品庫存的條件與分頁。
pub fn plan_stock_balances(
    query: &StockBalanceQuery,
) -> Result<(StockBalanceFilter, PageRequest), CoreError> {
    let page = page_of(query.page, query.page_size)?;
    let terms = non_empty(query.q.as_deref())
        .map(|q| q.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let filter = StockBalanceFilter {
        terms,
        category_id: parse_id(query_field::CATEGORY_ID, query.category_id.as_deref())?,
        in_stock: parse_bool(query_field::IN_STOCK, query.in_stock.as_deref())?,
        below_safety_stock: parse_bool(
            query_field::BELOW_SAFETY_STOCK,
            query.below_safety_stock.as_deref(),
        )?
        .unwrap_or(false),
        include_inactive: parse_bool(
            query_field::INCLUDE_INACTIVE,
            query.include_inactive.as_deref(),
        )?
        .unwrap_or(true),
    };
    Ok((filter, page))
}

fn to_balance(row: StockBalanceRow) -> StockBalance {
    let below = row.safety_stock.is_some_and(|s| row.qty_on_hand < s);
    StockBalance {
        product_id: row.product_id,
        code: row.code,
        name: row.name,
        spec: row.spec,
        unit_name: row.unit_name,
        is_active: row.is_active,
        qty_on_hand: qty4(row.qty_on_hand),
        stock_value_twd: money2(row.stock_value_twd),
        safety_stock: row.safety_stock.map(qty4),
        below_safety_stock: below,
    }
}

/// 商品庫存：未刪除的 `goods` 商品，依 `lower(code), id` 排序。需要 `record.read`。
pub async fn list_stock_balances(
    pool: &PgPool,
    ctx: &AuthContext,
    query: &StockBalanceQuery,
) -> Result<Page<StockBalance>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let (filter, page) = plan_stock_balances(query)?;
    let total = db::count_stock_balances(pool, &filter).await?;
    let rows = db::list_stock_balances(pool, &filter, page.limit(), page.offset()).await?;
    Ok(Page::new(
        rows.into_iter().map(to_balance).collect(),
        total_of(total),
        page,
    ))
}

fn to_lot(row: LotListRow) -> InventoryLot {
    InventoryLot {
        id: row.id,
        lot_no: row.lot_no,
        received_date: row.received_date,
        source_type: row.source_type,
        source_doc_id: row.source_doc_id,
        source_record_no: row.source_record_no,
        qty_in: qty4(row.qty_in),
        qty_remaining: qty4(row.qty_remaining),
        unit_cost_twd: qty4(row.unit_cost_twd),
    }
}

/// 商品 `product_id` 的未刪除批號，依先進先出順序（`received_date, id`）。需要 `record.read`。
///
/// 商品不存在或已刪除 → [`CoreError::NotFound`]；`service` 商品回傳空列表。
pub async fn list_product_lots(
    pool: &PgPool,
    ctx: &AuthContext,
    product_id: Id,
    query: &LotQuery,
) -> Result<Page<InventoryLot>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let page = page_of(query.page, query.page_size)?;
    let include_empty =
        parse_bool(query_field::INCLUDE_EMPTY, query.include_empty.as_deref())?.unwrap_or(false);
    let product = db::find_products(pool, &[product_id])
        .await?
        .into_iter()
        .find(|p| !p.deleted)
        .ok_or_else(|| CoreError::NotFound(PRODUCT_NOT_FOUND.into()))?;
    if product.product_type != GOODS {
        return Ok(Page::new(Vec::new(), 0, page));
    }
    let filter = LotListFilter {
        product_id,
        include_empty,
        q: non_empty(query.q.as_deref()),
    };
    let total = db::count_lots_of_product(pool, &filter).await?;
    let rows = db::list_lots_of_product(pool, &filter, page.limit(), page.offset()).await?;
    Ok(Page::new(
        rows.into_iter().map(to_lot).collect(),
        total_of(total),
        page,
    ))
}

/// 異動流水帳的條件與分頁。
pub fn plan_transactions(
    query: &TransactionQuery,
) -> Result<(TransactionFilter<'_>, PageRequest), CoreError> {
    let page = page_of(query.page, query.page_size)?;
    let date_from = parse_date(query_field::DATE_FROM, query.date_from.as_deref())?;
    let date_to = parse_date(query_field::DATE_TO, query.date_to.as_deref())?;
    if let (Some(from), Some(to)) = (date_from, date_to)
        && from > to
    {
        return Err(invalid(query_field::DATE_FROM, "開始日期不可晚於結束日期"));
    }
    let txn_type = non_empty(query.txn_type.as_deref());
    if let Some(t) = txn_type
        && TxnType::parse(t).is_none()
    {
        return Err(invalid(
            query_field::TXN_TYPE,
            format!("不支援的異動類型：{t}"),
        ));
    }
    let filter = TransactionFilter {
        product_id: parse_id(query_field::PRODUCT_ID, query.product_id.as_deref())?,
        lot_id: parse_id(query_field::LOT_ID, query.lot_id.as_deref())?,
        date_from,
        date_to,
        txn_type,
        ref_doc_type: non_empty(query.ref_doc_type.as_deref()),
        ref_doc_id: parse_id(query_field::REF_DOC_ID, query.ref_doc_id.as_deref())?,
    };
    Ok((filter, page))
}

fn to_transaction(row: TransactionListRow) -> Result<InventoryTransaction, CoreError> {
    let txn_type = TxnType::parse(&row.txn_type)
        .ok_or_else(|| CoreError::Internal(format!("不認得的異動類型：{}", row.txn_type)))?;
    Ok(InventoryTransaction {
        id: row.id,
        txn_at: row.txn_at,
        txn_date: row.txn_date,
        txn_type,
        product_id: row.product_id,
        product_code: row.product_code,
        product_name: row.product_name,
        lot_id: row.lot_id,
        lot_no: row.lot_no,
        qty_change: qty4(row.qty_change),
        unit_cost_twd: qty4(row.unit_cost_twd),
        ref_doc_type: row.ref_doc_type,
        ref_doc_id: row.ref_doc_id,
        ref_record_no: row.ref_record_no,
        ref_line_id: row.ref_line_id,
        created_by: row.created_by,
        created_by_name: row.created_by_name,
    })
}

/// 異動流水帳，依 `txn_date DESC, id DESC`（新到舊）。需要 `record.read`。不回傳累計餘額。
pub async fn list_transactions(
    pool: &PgPool,
    ctx: &AuthContext,
    query: &TransactionQuery,
) -> Result<Page<InventoryTransaction>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let (filter, page) = plan_transactions(query)?;
    let total = db::count_transactions(pool, &filter).await?;
    let rows = db::list_transactions(pool, &filter, page.limit(), page.offset()).await?;
    let items = rows
        .into_iter()
        .map(to_transaction)
        .collect::<Result<_, _>>()?;
    Ok(Page::new(items, total_of(total), page))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn field_of(err: CoreError) -> &'static str {
        match err {
            CoreError::InvalidField { field, .. } => field,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn money_is_rounded_half_away_from_zero() {
        assert_eq!(money2(d("250.3085")).to_string(), "250.31");
        assert_eq!(money2(d("0.005")).to_string(), "0.01");
        assert_eq!(money2(d("165")).to_string(), "165.00");
        assert_eq!(money2(Decimal::ZERO).to_string(), "0.00");
    }

    #[test]
    fn balance_defaults_and_terms() {
        let (filter, page) = plan_stock_balances(&StockBalanceQuery::default()).unwrap();
        assert_eq!(page, PageRequest::default());
        assert_eq!(
            filter,
            StockBalanceFilter {
                include_inactive: true,
                ..StockBalanceFilter::default()
            }
        );
        let query = StockBalanceQuery {
            q: Some("  螺絲  M3 ".into()),
            in_stock: Some("false".into()),
            below_safety_stock: Some("true".into()),
            include_inactive: Some("false".into()),
            category_id: Some(" ".into()),
            ..StockBalanceQuery::default()
        };
        let (filter, _) = plan_stock_balances(&query).unwrap();
        assert_eq!(filter.terms, ["螺絲", "M3"]);
        assert_eq!(filter.in_stock, Some(false));
        assert!(filter.below_safety_stock);
        assert!(!filter.include_inactive);
        assert_eq!(filter.category_id, None);
    }

    #[test]
    fn balance_params_must_be_valid() {
        for (query, field) in [
            (
                StockBalanceQuery {
                    category_id: Some("x".into()),
                    ..StockBalanceQuery::default()
                },
                query_field::CATEGORY_ID,
            ),
            (
                StockBalanceQuery {
                    in_stock: Some("yes".into()),
                    ..StockBalanceQuery::default()
                },
                query_field::IN_STOCK,
            ),
            (
                StockBalanceQuery {
                    page: Some(0),
                    ..StockBalanceQuery::default()
                },
                "page",
            ),
        ] {
            assert_eq!(field_of(plan_stock_balances(&query).unwrap_err()), field);
        }
    }

    #[test]
    fn transaction_params_are_parsed() {
        let id = Id::from_u128(7);
        let query = TransactionQuery {
            product_id: Some(id.to_string()),
            date_from: Some("2026-11-01".into()),
            date_to: Some("2026-11-01".into()),
            txn_type: Some("void_reversal".into()),
            ref_doc_type: Some("".into()),
            ..TransactionQuery::default()
        };
        let (filter, _) = plan_transactions(&query).unwrap();
        assert_eq!(filter.product_id, Some(id));
        assert_eq!(filter.date_from, Some(time::macros::date!(2026 - 11 - 01)));
        assert_eq!(filter.txn_type, Some("void_reversal"));
        assert_eq!(filter.ref_doc_type, None);
    }

    #[test]
    fn transaction_params_must_be_valid() {
        let q = |f: fn(&mut TransactionQuery)| {
            let mut query = TransactionQuery::default();
            f(&mut query);
            field_of(plan_transactions(&query).unwrap_err())
        };
        assert_eq!(q(|x| x.lot_id = Some("abc".into())), query_field::LOT_ID);
        assert_eq!(
            q(|x| x.ref_doc_id = Some("abc".into())),
            query_field::REF_DOC_ID
        );
        assert_eq!(
            q(|x| x.date_to = Some("2026/11/01".into())),
            query_field::DATE_TO
        );
        assert_eq!(
            q(|x| x.txn_type = Some("transfer".into())),
            query_field::TXN_TYPE
        );
        assert_eq!(
            q(|x| {
                x.date_from = Some("2026-11-02".into());
                x.date_to = Some("2026-11-01".into());
            }),
            query_field::DATE_FROM
        );
    }

    #[test]
    fn balance_marks_below_safety_stock() {
        let row = |qty: &str, safety: Option<&str>| StockBalanceRow {
            product_id: Id::nil(),
            code: "A".into(),
            name: "A".into(),
            spec: None,
            unit_name: "個".into(),
            is_active: true,
            qty_on_hand: d(qty),
            stock_value_twd: d("1.005"),
            safety_stock: safety.map(d),
        };
        let b = to_balance(row("3", Some("10")));
        assert!(b.below_safety_stock);
        assert_eq!(b.qty_on_hand.to_string(), "3.0000");
        assert_eq!(b.stock_value_twd.to_string(), "1.01");
        assert_eq!(b.safety_stock.unwrap().to_string(), "10.0000");
        assert!(!to_balance(row("10", Some("10"))).below_safety_stock);
        assert!(!to_balance(row("0", None)).below_safety_stock);
    }
}
