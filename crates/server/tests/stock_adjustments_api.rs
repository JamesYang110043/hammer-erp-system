//! 盤點調整單過帳、作廢的整合測試（#131，docs/architecture.md §8.10、§9.3）：期初庫存、盤盈、盤虧、
//! 庫存不足、作廢的反向處理與限制，以及存檔與過帳時的明細檢查。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use std::str::FromStr;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::decimal::Decimal;
use hammer_shared::document::{LotInUseDetails, StockInsufficientDetails};
use hammer_shared::record::Record;
use hammer_shared::time::today_taipei;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::Date;
use time::macros::date;
use tower::ServiceExt;

const DOC: &str = "stock_adjustment";
const LINE: &str = "stock_adjustment_line";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn state(pool: &PgPool) -> AppState {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    AppState::new(pool.clone(), config)
}

async fn call(
    pool: &PgPool,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(json) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let res = router(state(pool))
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

async fn editor(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("editor", role::EDITOR))
        .await
        .unwrap();
    token.into_string()
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

fn record_uri(entity: &str, id: Id) -> String {
    format!("/api/v1/entities/{entity}/records/{id}")
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 測試用的商品；`product_type` 為 `goods` 或 `service`。
async fn product(pool: &PgPool, token: &str, code: &str, product_type: &str) -> Id {
    let unit: Option<Id> = sqlx::query_scalar("SELECT id FROM units WHERE code = 'pcs'")
        .fetch_optional(pool)
        .await
        .unwrap();
    let unit = match unit {
        Some(id) => id,
        None => {
            create(
                pool,
                token,
                "unit",
                json!({ "data": { "code": "pcs", "name": "個" } }),
            )
            .await
            .id
        }
    };
    create(
        pool,
        token,
        "product",
        json!({ "data": {
            "code": code, "name": format!("商品 {code}"), "unit_id": unit, "product_type": product_type,
        } }),
    )
    .await
    .id
}

fn doc_body(adjustment_type: &str, lines: Value) -> Value {
    json!({
        "data": { "adjustment_date": "2026-11-01", "adjustment_type": adjustment_type },
        "lines": { LINE: lines },
    })
}

/// 新增草稿，回傳 (status, body)。
async fn try_create(pool: &PgPool, token: &str, body: Value) -> (StatusCode, Value) {
    call(pool, Method::POST, &records_uri(DOC), token, Some(body)).await
}

async fn draft(pool: &PgPool, token: &str, adjustment_type: &str, lines: Value) -> Record {
    create(pool, token, DOC, doc_body(adjustment_type, lines)).await
}

async fn act(pool: &PgPool, token: &str, record: &Record, action: &str) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("{}/{action}", record_uri(DOC, record.id)),
        token,
        Some(json!({ "version": record.version })),
    )
    .await
}

async fn act_ok(pool: &PgPool, token: &str, record: &Record, action: &str) -> Record {
    let (status, body) = act(pool, token, record, action).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(DOC, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

fn api_error(status: StatusCode, body: &Value, code: ErrorCode) -> ApiError {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, code, "{body}");
    err
}

/// `422 VALIDATION` 的 `details.lines`：`(index, field_key, message)`。
fn line_errors(status: StatusCode, body: &Value) -> Vec<(Option<u64>, Option<String>, String)> {
    let err = api_error(status, body, ErrorCode::Validation);
    err.details.unwrap()["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            assert_eq!(l["detail_key"], json!(LINE));
            (
                l["index"].as_u64(),
                l["field_key"].as_str().map(str::to_owned),
                l["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn le(index: u64, field: &str, message: &str) -> (Option<u64>, Option<String>, String) {
    (Some(index), Some(field.to_owned()), message.to_owned())
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct LotRow {
    id: Id,
    lot_no: String,
    received_date: Date,
    qty_in: Decimal,
    qty_remaining: Decimal,
    unit_cost_twd: Decimal,
    deleted: bool,
}

async fn lots(pool: &PgPool, product_id: Id) -> Vec<LotRow> {
    sqlx::query_as(
        "SELECT id, lot_no, received_date, qty_in, qty_remaining, unit_cost_twd,
                deleted_at IS NOT NULL AS deleted
         FROM inventory_lots WHERE product_id = $1 ORDER BY received_date, id",
    )
    .bind(product_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn lot_by_no(pool: &PgPool, product_id: Id, lot_no: &str) -> LotRow {
    lots(pool, product_id)
        .await
        .into_iter()
        .find(|l| l.lot_no == lot_no && !l.deleted)
        .unwrap()
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TxnRow {
    txn_date: Date,
    txn_type: String,
    lot_id: Id,
    qty_change: Decimal,
    unit_cost_twd: Decimal,
    ref_doc_id: Id,
    ref_line_id: Id,
}

async fn txns(pool: &PgPool, product_id: Id) -> Vec<TxnRow> {
    sqlx::query_as(
        "SELECT txn_date, txn_type, lot_id, qty_change, unit_cost_twd, ref_doc_id, ref_line_id
         FROM inventory_transactions WHERE product_id = $1 ORDER BY txn_at, id",
    )
    .bind(product_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// `v_stock_balances` 的 (數量, 金額)；沒有批號時為 `None`。
async fn balance(pool: &PgPool, product_id: Id) -> Option<(Decimal, Decimal)> {
    sqlx::query_as(
        "SELECT qty_on_hand, stock_value_twd FROM v_stock_balances WHERE product_id = $1",
    )
    .bind(product_id)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn line_lot_ids(pool: &PgPool, doc_id: Id) -> Vec<Option<Id>> {
    sqlx::query_scalar(
        "SELECT lot_id FROM stock_adjustment_lines
         WHERE stock_adjustment_id = $1 AND deleted_at IS NULL ORDER BY sort_order, id",
    )
    .bind(doc_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

fn lines_of(record: &Record) -> &[Record] {
    &record.lines.as_ref().unwrap()[LINE]
}

/// 期初庫存：兩個商品、自動產生與手動輸入的批號；過帳後有批號、異動、`v_stock_balances`，明細寫入批號。
async fn opening(pool: &PgPool, token: &str, a: Id, b: Id) -> Record {
    let record = draft(
        pool,
        token,
        "opening",
        json!([
            { "data": { "product_id": a, "qty": "10", "unit_cost_twd": "12.5" } },
            { "data": { "product_id": a, "lot_no": "  A-OLD ", "qty": "4", "unit_cost_twd": "10" } },
            { "data": { "product_id": b, "lot_no": "B-01", "qty": "2.5", "unit_cost_twd": "100.1234" } },
        ]),
    )
    .await;
    act_ok(pool, token, &record, "post").await
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn opening_stock_creates_lots_transactions_and_balances(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;
    let posted = opening(&pool, &token, a, b).await;
    let no = posted.record_no.clone().unwrap();
    let lines = lines_of(&posted);

    // 自動產生的批號寫回明細；手動輸入的去除空白
    assert_eq!(lines[0].data["lot_no"], json!(format!("{no}-01")));
    assert_eq!(lines[1].data["lot_no"], json!("A-OLD"));
    assert_eq!(lines[2].data["lot_no"], json!("B-01"));
    assert_eq!(lines[0].version, 2, "過帳寫入批號時明細的 version + 1");

    let a_lots = lots(&pool, a).await;
    assert_eq!(a_lots.len(), 2);
    let auto = a_lots
        .iter()
        .find(|l| l.lot_no == format!("{no}-01"))
        .unwrap();
    assert_eq!(
        (
            auto.received_date,
            auto.qty_in,
            auto.qty_remaining,
            auto.unit_cost_twd
        ),
        (date!(2026 - 11 - 01), d("10"), d("10"), d("12.5"))
    );
    let old = lot_by_no(&pool, a, "A-OLD").await;
    let b_lot = lot_by_no(&pool, b, "B-01").await;
    assert_eq!(
        line_lot_ids(&pool, posted.id).await,
        [Some(auto.id), Some(old.id), Some(b_lot.id)]
    );

    let a_txns = txns(&pool, a).await;
    assert_eq!(
        a_txns,
        [
            TxnRow {
                txn_date: date!(2026 - 11 - 01),
                txn_type: "adjustment".into(),
                lot_id: auto.id,
                qty_change: d("10"),
                unit_cost_twd: d("12.5"),
                ref_doc_id: posted.id,
                ref_line_id: lines[0].id,
            },
            TxnRow {
                txn_date: date!(2026 - 11 - 01),
                txn_type: "adjustment".into(),
                lot_id: old.id,
                qty_change: d("4"),
                unit_cost_twd: d("10"),
                ref_doc_id: posted.id,
                ref_line_id: lines[1].id,
            },
        ]
    );
    assert_eq!(balance(&pool, a).await, Some((d("14"), d("165"))));
    assert_eq!(balance(&pool, b).await, Some((d("2.5"), d("250.3085"))));
}

/// 盤盈加回既有批號、盤虧扣指定批號：數量、異動、批號扣除明細，明細寫入批號的成本。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn count_gain_and_loss_adjust_existing_lots(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;
    opening(&pool, &token, a, b).await;
    let old = lot_by_no(&pool, a, "A-OLD").await;

    let record = draft(
        &pool,
        &token,
        "count",
        json!([
            { "data": { "product_id": a, "line_type": "gain_existing", "lot_no": "a-old", "qty": "1.5", "reason": "盤點差異" } },
            { "data": { "product_id": b, "line_type": "loss", "lot_no": "B-01", "qty": "2.5", "reason": "報廢" } },
        ]),
    )
    .await;
    let posted = act_ok(&pool, &token, &record, "post").await;
    let lines = lines_of(&posted);
    assert_eq!(lines[0].data["unit_cost_twd"], json!("10.0000"));
    assert_eq!(lines[1].data["unit_cost_twd"], json!("100.1234"));

    assert_eq!(lot_by_no(&pool, a, "A-OLD").await.qty_remaining, d("5.5"));
    let b_lot = lot_by_no(&pool, b, "B-01").await;
    assert_eq!(b_lot.qty_remaining, Decimal::ZERO);
    assert_eq!(
        line_lot_ids(&pool, posted.id).await,
        [Some(old.id), Some(b_lot.id)]
    );
    let last = txns(&pool, b).await.pop().unwrap();
    assert_eq!(
        (last.txn_type.as_str(), last.qty_change, last.ref_line_id),
        ("adjustment", d("-2.5"), lines[1].id)
    );
    let allocations: Vec<(Id, Decimal, Decimal)> = sqlx::query_as(
        "SELECT lot_id, qty, unit_cost_twd FROM lot_allocations WHERE source_line_id = $1",
    )
    .bind(lines[1].id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(allocations, [(b_lot.id, d("2.5"), d("100.1234"))]);
    assert_eq!(balance(&pool, a).await, Some((d("15.5"), d("180"))));
    assert_eq!(balance(&pool, b).await, Some((d("0"), d("0"))));
}

/// 盤虧超過批號剩餘數量 → `STOCK_INSUFFICIENT`，列出所有不足，整張不寫入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn loss_over_remaining_is_rejected_and_nothing_is_written(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;
    let first = opening(&pool, &token, a, b).await;
    let auto_lot = format!("{}-01", first.record_no.as_deref().unwrap());
    let before = (
        lots(&pool, a).await,
        lots(&pool, b).await,
        count(&pool, "inventory_transactions").await,
        count(&pool, "lot_allocations").await,
    );

    let record = draft(
        &pool,
        &token,
        "adjustment",
        json!([
            // 足夠的列會先寫入，最後一起 rollback
            { "data": { "product_id": a, "line_type": "loss", "lot_no": "A-OLD", "qty": "1" } },
            { "data": { "product_id": a, "line_type": "gain_new", "lot_no": "A-NEW", "qty": "1", "unit_cost_twd": "1" } },
            { "data": { "product_id": b, "line_type": "loss", "lot_no": "B-01", "qty": "3" } },
            { "data": { "product_id": a, "line_type": "loss", "lot_no": auto_lot, "qty": "11" } },
        ]),
    )
    .await;
    let (status, body) = act(&pool, &token, &record, "post").await;
    let err = api_error(status, &body, ErrorCode::StockInsufficient);
    assert_eq!(
        err.message,
        "商品 B001 批號 B-01 庫存不足，尚缺 0.5 等 2 項"
    );
    let details: StockInsufficientDetails = serde_json::from_value(err.details.unwrap()).unwrap();
    type Item<'a> = (&'a str, Option<&'a str>, String, String, String, Vec<u32>);
    let items: Vec<Item> = details
        .items
        .iter()
        .map(|i| {
            (
                i.product_code.as_str(),
                i.lot_no.as_deref(),
                i.required.to_string(),
                i.available.to_string(),
                i.shortage.to_string(),
                i.lines.iter().map(|l| l.index).collect(),
            )
        })
        .collect();
    assert_eq!(
        items,
        [
            (
                "B001",
                Some("B-01"),
                "3.0000".into(),
                "2.5000".into(),
                "0.5000".into(),
                vec![2]
            ),
            (
                "A001",
                Some(auto_lot.as_str()),
                "11.0000".into(),
                "10.0000".into(),
                "1.0000".into(),
                vec![3]
            ),
        ]
    );

    // 整張不寫入：批號、異動、批號扣除明細、單據與明細都不變
    let after = (
        lots(&pool, a).await,
        lots(&pool, b).await,
        count(&pool, "inventory_transactions").await,
        count(&pool, "lot_allocations").await,
    );
    assert_eq!(after, before);
    assert_eq!(get(&pool, &token, record.id).await, record);
    assert_eq!(line_lot_ids(&pool, record.id).await, [None; 4]);
}

/// 作廢：盤盈新批號刪除、加回的數量減回、盤虧加回原批號；庫存回到過帳前。異動為 `void_reversal`、
/// 日期為作廢當天。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_restores_stock(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;
    let first = opening(&pool, &token, a, b).await;
    let before = (
        lots(&pool, a).await,
        lots(&pool, b).await,
        balance(&pool, a).await,
        balance(&pool, b).await,
    );

    let record = draft(
        &pool,
        &token,
        "count",
        json!([
            { "data": { "product_id": a, "line_type": "gain_existing", "lot_no": "A-OLD", "qty": "1.5" } },
            { "data": { "product_id": b, "line_type": "loss", "lot_no": "B-01", "qty": "1" } },
            { "data": { "product_id": a, "line_type": "gain_new", "lot_no": "A-NEW", "qty": "2", "unit_cost_twd": "3" } },
        ]),
    )
    .await;
    let posted = act_ok(&pool, &token, &record, "post").await;
    assert_eq!(balance(&pool, a).await, Some((d("17.5"), d("186"))));
    let new_lot = lot_by_no(&pool, a, "A-NEW").await;

    let voided = act_ok(&pool, &token, &posted, "void").await;
    assert_eq!(voided.data["status"], json!("void"));
    let mut a_lots = lots(&pool, a).await;
    let removed = a_lots.iter().position(|l| l.id == new_lot.id).unwrap();
    let removed = a_lots.remove(removed);
    assert!(removed.deleted);
    assert_eq!(removed.qty_remaining, Decimal::ZERO);
    let after = (
        a_lots,
        lots(&pool, b).await,
        balance(&pool, a).await,
        balance(&pool, b).await,
    );
    assert_eq!(after, before);

    let today = today_taipei();
    let reversals: Vec<(Id, Decimal, Decimal)> = txns(&pool, a)
        .await
        .into_iter()
        .chain(txns(&pool, b).await)
        .filter(|t| t.txn_type == "void_reversal")
        .map(|t| {
            assert_eq!((t.txn_date, t.ref_doc_id), (today, record.id));
            (t.ref_line_id, t.qty_change, t.unit_cost_twd)
        })
        .collect();
    let lines = lines_of(&posted);
    assert_eq!(reversals.len(), 3);
    for expected in [
        (lines[0].id, d("-1.5"), d("10")),
        (lines[2].id, d("-2"), d("3")),
        (lines[1].id, d("1"), d("100.1234")),
    ] {
        assert!(reversals.contains(&expected), "{reversals:?}");
    }

    // 期初庫存也可以作廢：所有批號刪除
    let first = get(&pool, &token, first.id).await;
    act_ok(&pool, &token, &first, "void").await;
    assert!(lots(&pool, a).await.iter().all(|l| l.deleted));
    assert_eq!(balance(&pool, a).await, None);
    assert_eq!(balance(&pool, b).await, None);
}

/// 新批號已被其他單據扣用時作廢被拒絕（`LOT_IN_USE`）；先作廢後續單據後即可作廢。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_is_rejected_when_new_lot_is_used(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;
    let first = opening(&pool, &token, a, b).await;
    let loss = draft(
        &pool,
        &token,
        "count",
        json!([{ "data": { "product_id": a, "line_type": "loss", "lot_no": "A-OLD", "qty": "1" } }]),
    )
    .await;
    let loss = act_ok(&pool, &token, &loss, "post").await;
    let before = (
        lots(&pool, a).await,
        count(&pool, "inventory_transactions").await,
    );

    let (status, body) = act(&pool, &token, &first, "void").await;
    let err = api_error(status, &body, ErrorCode::LotInUse);
    let loss_no = loss.record_no.clone().unwrap();
    assert_eq!(
        err.message,
        format!("批號 A-OLD 已被其他單據使用，請先作廢 {loss_no}")
    );
    let details: LotInUseDetails = serde_json::from_value(err.details.unwrap()).unwrap();
    assert_eq!(details.lots.len(), 1);
    assert_eq!(details.lots[0].product_code, "A001");
    assert_eq!(details.lots[0].documents[0].record_id, loss.id);
    assert_eq!(get(&pool, &token, first.id).await, first);
    assert_eq!(
        (
            lots(&pool, a).await,
            count(&pool, "inventory_transactions").await
        ),
        before
    );

    act_ok(&pool, &token, &loss, "void").await;
    act_ok(&pool, &token, &first, "void").await;
    assert_eq!(balance(&pool, a).await, None);
}

/// 加回既有批號的數量已被用掉時作廢被拒絕（`STOCK_INSUFFICIENT`）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_gain_existing_requires_remaining_qty(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;
    opening(&pool, &token, a, b).await;
    let gain = draft(
        &pool,
        &token,
        "count",
        json!([{ "data": { "product_id": a, "line_type": "gain_existing", "lot_no": "A-OLD", "qty": "3" } }]),
    )
    .await;
    let gain = act_ok(&pool, &token, &gain, "post").await;
    let loss = draft(
        &pool,
        &token,
        "count",
        json!([{ "data": { "product_id": a, "line_type": "loss", "lot_no": "A-OLD", "qty": "6" } }]),
    )
    .await;
    act_ok(&pool, &token, &loss, "post").await;
    assert_eq!(lot_by_no(&pool, a, "A-OLD").await.qty_remaining, d("1"));

    let (status, body) = act(&pool, &token, &gain, "void").await;
    let err = api_error(status, &body, ErrorCode::StockInsufficient);
    assert_eq!(err.message, "商品 A001 批號 A-OLD 庫存不足，尚缺 2");
    assert_eq!(get(&pool, &token, gain.id).await, gain);
    assert_eq!(lot_by_no(&pool, a, "A-OLD").await.qty_remaining, d("1"));
}

/// 服務類商品不可加入調整單：新增、修改時擋下；存檔後商品改為服務類時過帳擋下。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn service_products_are_rejected(pool: PgPool) {
    let token = editor(&pool).await;
    let goods = product(&pool, &token, "A001", "goods").await;
    let service = product(&pool, &token, "S001", "service").await;
    let line = |p: Id| json!({ "data": { "product_id": p, "qty": "1", "unit_cost_twd": "1" } });

    let (status, body) = try_create(
        &pool,
        &token,
        doc_body("count", json!([line(goods), line(service)])),
    )
    .await;
    assert_eq!(
        line_errors(status, &body),
        [le(1, "product_id", "服務類商品沒有庫存")]
    );

    let record = draft(&pool, &token, "count", json!([line(goods)])).await;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &token,
        Some(json!({ "version": record.version, "lines": { LINE: [
            { "id": lines_of(&record)[0].id, "data": {} },
            line(service),
        ] } })),
    )
    .await;
    assert_eq!(
        line_errors(status, &body),
        [le(1, "product_id", "服務類商品沒有庫存")]
    );

    // 存檔後商品改為服務類 → 過帳時重新檢查
    let (status, body) = call(
        &pool,
        Method::GET,
        &record_uri("product", goods),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("product", goods),
        &token,
        Some(json!({ "version": body["version"], "data": { "product_type": "service" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = act(&pool, &token, &record, "post").await;
    assert_eq!(
        line_errors(status, &body),
        [le(0, "product_id", "服務類商品沒有庫存")]
    );
    assert_eq!(get(&pool, &token, record.id).await, record);
    assert_eq!(count(&pool, "inventory_lots").await, 0);
}

/// 存檔時的明細檢查（§8.10「存檔時的檢查」）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn save_checks_lines(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;
    opening(&pool, &token, a, b).await;

    let (status, body) = try_create(
        &pool,
        &token,
        doc_body(
            "count",
            json!([
                { "data": { "product_id": a, "qty": "1" } },
                { "data": { "product_id": a, "line_type": "gain_existing", "unit_cost_twd": "1", "qty": "1" } },
                { "data": { "product_id": a, "line_type": "loss", "lot_no": "NOPE", "qty": "1" } },
                { "data": { "product_id": a, "lot_no": "a-old", "qty": "1", "unit_cost_twd": "1" } },
                { "data": { "product_id": b, "line_type": "loss", "lot_no": "B-01", "qty": "1" } },
                { "data": { "product_id": b, "line_type": "gain_existing", "lot_no": "b-01", "qty": "1" } },
                { "data": { "product_id": b, "lot_no": "B-02", "qty": "0", "unit_cost_twd": "1" } },
            ]),
        ),
    )
    .await;
    assert_eq!(
        line_errors(status, &body),
        [
            le(0, "unit_cost_twd", "此欄位為必填"),
            le(1, "lot_no", "此欄位為必填"),
            le(1, "unit_cost_twd", "既有批號的成本不可修改"),
            le(6, "qty", "數量必須大於 0"),
            le(2, "lot_no", "找不到此批號"),
            le(3, "lot_no", "批號已存在"),
            le(5, "lot_no", "批號重複"),
        ]
    );

    // 期初庫存只能新增批號
    let loss =
        json!({ "data": { "product_id": a, "line_type": "loss", "lot_no": "A-OLD", "qty": "1" } });
    let (status, body) = try_create(&pool, &token, doc_body("opening", json!([loss]))).await;
    assert_eq!(
        line_errors(status, &body),
        [le(0, "line_type", "期初庫存只能新增批號")]
    );
    // 修改時只改調整類型也會以既有明細檢查
    let record = draft(&pool, &token, "count", json!([loss])).await;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &token,
        Some(json!({ "version": record.version, "data": { "adjustment_type": "opening" } })),
    )
    .await;
    assert_eq!(
        line_errors(status, &body),
        [le(0, "line_type", "期初庫存只能新增批號")]
    );
    // 修改其他欄位不受影響
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &token,
        Some(json!({ "version": record.version, "data": { "notes": "x" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 過帳時重新檢查：沒有明細、存檔後批號被作廢刪除、存檔後才建立的同名批號。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn post_rechecks_lines(pool: PgPool) {
    let token = editor(&pool).await;
    let a = product(&pool, &token, "A001", "goods").await;
    let b = product(&pool, &token, "B001", "goods").await;

    let empty = create(
        &pool,
        &token,
        DOC,
        json!({ "data": { "adjustment_date": "2026-11-01" } }),
    )
    .await;
    let (status, body) = act(&pool, &token, &empty, "post").await;
    assert_eq!(
        line_errors(status, &body),
        [(None, None, "至少要有一筆明細".to_owned())]
    );

    let first = opening(&pool, &token, a, b).await;
    let loss = draft(
        &pool,
        &token,
        "count",
        json!([{ "data": { "product_id": b, "line_type": "loss", "lot_no": "B-01", "qty": "1" } }]),
    )
    .await;
    let gain = draft(
        &pool,
        &token,
        "count",
        json!([{ "data": { "product_id": a, "lot_no": "A-LATER", "qty": "1", "unit_cost_twd": "1" } }]),
    )
    .await;
    let other = draft(
        &pool,
        &token,
        "count",
        json!([{ "data": { "product_id": a, "lot_no": "a-later", "qty": "1", "unit_cost_twd": "1" } }]),
    )
    .await;

    act_ok(&pool, &token, &first, "void").await;
    let (status, body) = act(&pool, &token, &loss, "post").await;
    assert_eq!(
        line_errors(status, &body),
        [le(0, "lot_no", "找不到此批號")]
    );

    act_ok(&pool, &token, &gain, "post").await;
    let (status, body) = act(&pool, &token, &other, "post").await;
    assert_eq!(line_errors(status, &body), [le(0, "lot_no", "批號已存在")]);
    assert_eq!(get(&pool, &token, other.id).await, other);
}
