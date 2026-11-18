//! 庫存查詢（docs/architecture.md §8.10「庫存查詢 API」、§10 分頁）。型別在 [`hammer_shared::inventory`]。
//!
//! - [`list_stock_balances`]：商品庫存（數量、台幣庫存金額、低於安全庫存）
//! - [`list_product_lots`]：某商品的批號（先進先出順序）
//! - [`list_transactions`]：異動流水帳（新到舊）
//! - [`allocation_preview`]：出貨的建議批號（先進先出試算，§8.9「建議批號 API」）
//!
//! 都需要 `record.read`。篩選參數格式錯誤 → [`CoreError::InvalidField`]（`422 VALIDATION`，
//! `details.field` 為參數名稱）；空字串的參數視為未指定。

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use hammer_db::inventory::{
    self as db, LotListFilter, LotListRow, StockBalanceFilter, StockBalanceRow, TransactionFilter,
    TransactionListRow,
};
use hammer_db::sqlx::PgPool;
use hammer_shared::decimal::{Decimal, RoundingStrategy};
use hammer_shared::inventory::{
    ALLOCATION_PREVIEW_MAX_LINES, AllocationPreview, AllocationPreviewLine,
    AllocationPreviewRequest, InventoryLot, InventoryTransaction, LotQuery, PreviewAllocation,
    StockBalance, StockBalanceQuery, TransactionQuery, TxnType, query_field,
};
use hammer_shared::pagination::{Page, PageRequest};
use hammer_shared::{Id, permission};
use time::Date;
use time::macros::format_description;

use crate::CoreError;
use crate::auth::AuthContext;
use crate::inventory::{
    GOODS, PreviewLine, PreviewLot, normalize_lot_no, preview_allocations, qty4,
};
use crate::record_lines::LineError;
use crate::stock_adjustments::MSG_LOT_NOT_FOUND;

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

/// 建議批號：商品不存在或已刪除。
pub const MSG_PREVIEW_PRODUCT: &str = "商品不存在";
/// 建議批號：列數不在 1–200。
pub const MSG_PREVIEW_LINES: &str = "明細必須有 1 到 200 列";
const MSG_REQUIRED: &str = "此欄位為必填";
const MSG_ID_FORMAT: &str = "不是有效的 id";
const MSG_QTY_FORMAT: &str = "必須是數字";
const MSG_QTY_SCALE: &str = "最多 4 位小數";
const MSG_QTY_POSITIVE: &str = "數量必須大於 0";

/// 解析後的一列。
struct ParsedLine {
    product_id: Id,
    qty: Decimal,
    lot_no: Option<String>,
}

fn preview_error(index: usize, field: &str, message: impl Into<String>) -> LineError {
    // detail_key 為空字串：API 回應為 null（不屬於明細子表）
    LineError::field("", index, field, message)
}

/// 解析 request 的一列；格式錯誤時回傳該列的所有錯誤。
fn parse_preview_line(
    index: usize,
    line: &hammer_shared::inventory::AllocationPreviewLineInput,
) -> Result<ParsedLine, Vec<LineError>> {
    let mut errors = Vec::new();
    let product_id = match non_empty(line.product_id.as_deref()) {
        None => {
            errors.push(preview_error(index, "product_id", MSG_REQUIRED));
            None
        }
        Some(s) => match Id::parse_str(s) {
            Ok(id) => Some(id),
            Err(_) => {
                errors.push(preview_error(index, "product_id", MSG_ID_FORMAT));
                None
            }
        },
    };
    let qty = match non_empty(line.qty.as_deref()) {
        None => {
            errors.push(preview_error(index, "qty", MSG_REQUIRED));
            None
        }
        Some(s) => match Decimal::from_str(s) {
            Err(_) => {
                errors.push(preview_error(index, "qty", MSG_QTY_FORMAT));
                None
            }
            Ok(q) if q.normalize().scale() > 4 => {
                errors.push(preview_error(index, "qty", MSG_QTY_SCALE));
                None
            }
            Ok(q) if q <= Decimal::ZERO => {
                errors.push(preview_error(index, "qty", MSG_QTY_POSITIVE));
                None
            }
            Ok(q) => Some(q),
        },
    };
    let lot_no = match line.lot_no.as_deref().map(normalize_lot_no) {
        None | Some(Ok(None)) => None,
        Some(Ok(Some(no))) => Some(no),
        Some(Err(message)) => {
            errors.push(preview_error(index, "lot_no", message));
            None
        }
    };
    match (product_id, qty) {
        (Some(product_id), Some(qty)) if errors.is_empty() => Ok(ParsedLine {
            product_id,
            qty,
            lot_no,
        }),
        _ => Err(errors),
    }
}

fn invalid_lines(lines: Vec<LineError>) -> CoreError {
    CoreError::InvalidLines {
        fields: Vec::new(),
        lines,
    }
}

/// 出貨的建議批號（`POST /api/v1/inventory/allocation-preview`）。需要 `record.read`。
///
/// 只試算：不鎖定批號、不寫入任何資料、不考慮其他草稿；分配邏輯與出貨過帳共用
/// （[`crate::inventory::preview_allocations`]、[`crate::inventory::take_fifo`]）。
///
/// - 列數不是 1–200、格式錯誤、商品不存在或已刪除（`product_id`「商品不存在」）、批號找不到
///   （`lot_no`「找不到此批號」）→ [`CoreError::InvalidLines`]（`detail_key` 為 `null`）
/// - 庫存不足不是錯誤：回傳能分配的部分與 `shortage`；`service` 商品的 `allocations` 為空陣列
pub async fn allocation_preview(
    pool: &PgPool,
    ctx: &AuthContext,
    req: &AllocationPreviewRequest,
) -> Result<AllocationPreview, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    if req.lines.is_empty() || req.lines.len() > ALLOCATION_PREVIEW_MAX_LINES {
        return Err(invalid_lines(vec![LineError::detail(
            "",
            MSG_PREVIEW_LINES,
        )]));
    }
    let mut errors = Vec::new();
    let mut parsed = Vec::with_capacity(req.lines.len());
    for (index, line) in req.lines.iter().enumerate() {
        match parse_preview_line(index, line) {
            Ok(p) => parsed.push(p),
            Err(e) => errors.extend(e),
        }
    }
    if !errors.is_empty() {
        return Err(invalid_lines(errors));
    }

    let mut conn = pool.acquire().await.map_err(hammer_db::DbError::from)?;
    let product_ids: Vec<Id> = parsed
        .iter()
        .map(|l| l.product_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let products: BTreeMap<Id, String> = db::find_products(&mut *conn, &product_ids)
        .await?
        .into_iter()
        .filter(|p| !p.deleted)
        .map(|p| (p.id, p.product_type))
        .collect();
    let mut lot_ids = Vec::with_capacity(parsed.len());
    for (index, line) in parsed.iter().enumerate() {
        let Some(product_type) = products.get(&line.product_id) else {
            errors.push(preview_error(index, "product_id", MSG_PREVIEW_PRODUCT));
            lot_ids.push(None);
            continue;
        };
        let lot_id = match (&line.lot_no, product_type == GOODS) {
            (Some(lot_no), true) => {
                match db::find_lot_by_no(&mut *conn, line.product_id, lot_no).await? {
                    Some(lot) => Some(lot.id),
                    None => {
                        errors.push(preview_error(index, "lot_no", MSG_LOT_NOT_FOUND));
                        None
                    }
                }
            }
            // 服務類商品沒有庫存，忽略批號
            _ => None,
        };
        lot_ids.push(lot_id);
    }
    if !errors.is_empty() {
        return Err(invalid_lines(errors));
    }

    let lines: Vec<PreviewLine> = parsed
        .iter()
        .zip(&lot_ids)
        .map(|(l, lot_id)| PreviewLine {
            product_id: l.product_id,
            is_goods: products.get(&l.product_id).is_some_and(|t| t == GOODS),
            qty: l.qty,
            lot_id: *lot_id,
        })
        .collect();
    let fifo_products: Vec<Id> = lines
        .iter()
        .filter(|l| l.is_goods && l.lot_id.is_none())
        .map(|l| l.product_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let explicit: Vec<Id> = lot_ids.iter().flatten().copied().collect();
    let rows = db::list_lots_for_preview(&mut *conn, &explicit, &fifo_products).await?;
    let lots: Vec<PreviewLot> = rows
        .iter()
        .map(|r| PreviewLot {
            id: r.id,
            product_id: r.product_id,
            received_date: r.received_date,
            qty_remaining: r.qty_remaining,
        })
        .collect();
    let by_id: BTreeMap<Id, &hammer_db::inventory::LotRow> =
        rows.iter().map(|r| (r.id, r)).collect();

    let results = preview_allocations(&lines, &lots);
    let mut out = Vec::with_capacity(lines.len());
    for (index, (line, (taken, shortage))) in lines.iter().zip(results).enumerate() {
        let mut allocations = Vec::with_capacity(taken.len());
        for (lot_id, qty) in taken {
            let lot = by_id
                .get(&lot_id)
                .ok_or_else(|| CoreError::Internal(format!("批號 {lot_id} 沒有讀取")))?;
            allocations.push(PreviewAllocation {
                lot_id,
                lot_no: lot.lot_no.clone(),
                received_date: lot.received_date,
                qty: qty4(qty),
                unit_cost_twd: qty4(lot.unit_cost_twd),
            });
        }
        out.push(AllocationPreviewLine {
            index: u32::try_from(index).map_err(|_| CoreError::Internal("列數過多".into()))?,
            product_id: line.product_id,
            product_type: products.get(&line.product_id).cloned().unwrap_or_default(),
            qty: qty4(line.qty),
            allocations,
            shortage: qty4(shortage),
        });
    }
    Ok(AllocationPreview { lines: out })
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
