//! 庫存引擎的資料庫整合測試（#130，docs/architecture.md §8.10、§9）：入庫、先進先出與指定批號扣庫存、
//! 不足、反向處理、刪除批號、`service` 商品，以及兩個 transaction 同時扣同一商品不會超扣。
//!
//! 使用 `#[sqlx::test]`，執行前需設定 `DATABASE_URL`，見 README「整合測試」。

#![allow(clippy::unwrap_used)]

use std::str::FromStr;
use std::time::Duration;

use hammer_core::CoreError;
use hammer_core::inventory::{
    Inventory, LOT_NO_TAKEN, Line, LockRequest, Receipt, Source, allocations_of_line,
};
use hammer_db::sqlx::{self, PgConnection, PgPool};
use hammer_shared::decimal::Decimal;
use hammer_shared::document::LineRef;
use hammer_shared::inventory::TxnType;
use hammer_shared::{Id, new_id, role};
use time::Date;
use time::macros::date;

const DOC: &str = "stock_adjustment";
const LINE: &str = "stock_adjustment_line";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn user(pool: &PgPool) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, role_id)
         SELECT $1, 'editor', 'editor', '!', id FROM roles WHERE code = $2",
    )
    .bind(id)
    .bind(role::EDITOR)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn product(pool: &PgPool, code: &str, product_type: &str) -> Id {
    let unit: Id = match sqlx::query_scalar("SELECT id FROM units WHERE code = 'pcs'")
        .fetch_optional(pool)
        .await
        .unwrap()
    {
        Some(id) => id,
        None => {
            let id = new_id();
            sqlx::query("INSERT INTO units (id, code, name) VALUES ($1, 'pcs', '個')")
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
            id
        }
    };
    let id = new_id();
    sqlx::query(
        "INSERT INTO products (id, code, name, product_type, unit_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(code)
    .bind(format!("商品 {code}"))
    .bind(product_type)
    .bind(unit)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// 新增一張盤點調整單（只用於異動的來源與單號）。
async fn document(pool: &PgPool, record_no: &str) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO stock_adjustments (id, record_no, adjustment_date) VALUES ($1, $2, '2026-11-01')",
    )
    .bind(id)
    .bind(record_no)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn source(doc_id: Id, actor: Id, txn_date: Date) -> Source<'static> {
    Source {
        doc_type: DOC,
        line_type: LINE,
        doc_id,
        txn_date,
        actor,
    }
}

fn line(index: u32) -> Line {
    Line {
        id: new_id(),
        index,
    }
}

fn receipt<'a>(
    product_id: Id,
    lot_no: Option<&'a str>,
    record_no: &'a str,
    sort_order: i32,
    received_date: Date,
    qty: &str,
    cost: &str,
) -> Receipt<'a> {
    Receipt {
        product_id,
        lot_no,
        record_no,
        sort_order,
        received_date,
        qty: d(qty),
        unit_cost_twd: d(cost),
        txn_type: TxnType::Adjustment,
    }
}

/// 以一張單據入庫多個批號並 commit，回傳批號 id（依 `lots` 順序）。
async fn stock(
    pool: &PgPool,
    actor: Id,
    product_id: Id,
    record_no: &str,
    lots: &[(&str, Date, &str, &str)],
) -> Vec<Id> {
    let doc = document(pool, record_no).await;
    let src = source(doc, actor, date!(2026 - 11 - 01));
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![product_id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut ids = Vec::new();
    for (i, (no, received, qty, cost)) in lots.iter().enumerate() {
        let lot = inv
            .receive(
                &mut tx,
                &src,
                line(i as u32),
                &receipt(
                    product_id,
                    Some(no),
                    record_no,
                    i as i32,
                    *received,
                    qty,
                    cost,
                ),
            )
            .await
            .unwrap()
            .unwrap();
        ids.push(lot.id);
    }
    inv.finish().unwrap();
    tx.commit().await.unwrap();
    ids
}

async fn remaining(pool: &PgPool, lot_id: Id) -> Decimal {
    sqlx::query_scalar("SELECT qty_remaining FROM inventory_lots WHERE id = $1")
        .bind(lot_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count(pool: &PgPool, sql: &'static str, doc: Id) -> i64 {
    sqlx::query_scalar(sql)
        .bind(doc)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn txns(pool: &PgPool, doc: Id) -> Vec<(String, Id, Decimal, Decimal, Date)> {
    sqlx::query_as(
        "SELECT txn_type, lot_id, qty_change, unit_cost_twd, txn_date
         FROM inventory_transactions WHERE ref_doc_id = $1 ORDER BY id",
    )
    .bind(doc)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn fifo_lock(conn: &mut PgConnection, product_id: Id) -> Inventory {
    Inventory::lock(
        conn,
        &LockRequest {
            product_ids: vec![product_id],
            fifo_product_ids: vec![product_id],
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

/// 入庫建立批號與異動；批號留空時自動編號；同商品的批號不分大小寫不可重複。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn receive_creates_lots_and_transactions(pool: PgPool) {
    let actor = user(&pool).await;
    let p = product(&pool, "A001", "goods").await;
    let doc = document(&pool, "ADJ202611-0001").await;
    let src = source(doc, actor, date!(2026 - 11 - 02));

    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![p],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let manual = inv
        .receive(
            &mut tx,
            &src,
            line(0),
            &receipt(
                p,
                Some("  L-01 "),
                "ADJ202611-0001",
                0,
                date!(2026 - 11 - 02),
                "5",
                "12.5",
            ),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(manual.lot_no, "L-01");
    assert_eq!((manual.qty_in, manual.qty_remaining), (d("5"), d("5")));
    let auto = inv
        .receive(
            &mut tx,
            &src,
            line(1),
            &receipt(
                p,
                None,
                "ADJ202611-0001",
                1,
                date!(2026 - 11 - 02),
                "3",
                "10",
            ),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(auto.lot_no, "ADJ202611-0001-02");
    let blank = inv
        .receive(
            &mut tx,
            &src,
            line(2),
            &receipt(
                p,
                Some(" "),
                "ADJ202611-0001",
                11,
                date!(2026 - 11 - 02),
                "1",
                "10",
            ),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(blank.lot_no, "ADJ202611-0001-12");
    assert_eq!(inv.qty_on_hand(p), d("9"));
    assert_eq!(inv.lot_by_no(p, "l-01").unwrap().id, manual.id);
    inv.finish().unwrap();
    tx.commit().await.unwrap();

    let rows = txns(&pool, doc).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        (
            "adjustment".into(),
            manual.id,
            d("5"),
            d("12.5"),
            date!(2026 - 11 - 02)
        )
    );

    // 重複的批號（不分大小寫）
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![p],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let err = inv
        .receive(
            &mut tx,
            &src,
            line(0),
            &receipt(p, Some("l-01"), "X", 0, date!(2026 - 11 - 02), "1", "1"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CoreError::Validation(m) if m == LOT_NO_TAKEN),
        "{err:?}"
    );
}

/// 先進先出依入庫日期由舊到新、跨多個批號扣減；每筆扣減都有批號扣除明細與異動；剛好扣完。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn fifo_issues_across_lots(pool: PgPool) {
    let actor = user(&pool).await;
    let p = product(&pool, "A001", "goods").await;
    // 後建立但入庫日期較早的批號排在前面
    let lots = stock(
        &pool,
        actor,
        p,
        "ADJ-1",
        &[
            ("L-NEW", date!(2026 - 11 - 05), "10", "20"),
            ("L-OLD", date!(2026 - 10 - 01), "3", "10"),
            ("L-MID", date!(2026 - 10 - 15), "4", "15"),
        ],
    )
    .await;
    let (new, old, mid) = (lots[0], lots[1], lots[2]);

    let doc = document(&pool, "ADJ-2").await;
    let src = source(doc, actor, date!(2026 - 11 - 10));
    let mut tx = pool.begin().await.unwrap();
    let mut inv = fifo_lock(&mut tx, p).await;
    let first = line(0);
    let allocations = inv
        .issue_fifo(&mut tx, &src, first, p, d("8"), TxnType::Adjustment)
        .await
        .unwrap();
    let got: Vec<(Id, Decimal, Decimal)> = allocations
        .iter()
        .map(|a| (a.lot_id, a.qty, a.unit_cost_twd))
        .collect();
    assert_eq!(
        got,
        [
            (old, d("3"), d("10")),
            (mid, d("4"), d("15")),
            (new, d("1"), d("20"))
        ]
    );
    // 剛好扣完剩下的 9
    let rest = inv
        .issue_fifo(&mut tx, &src, line(1), p, d("9"), TxnType::Adjustment)
        .await
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(inv.qty_on_hand(p), Decimal::ZERO);
    inv.finish().unwrap();
    tx.commit().await.unwrap();

    for lot in [old, mid, new] {
        assert_eq!(remaining(&pool, lot).await, Decimal::ZERO);
    }
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM lot_allocations WHERE source_doc_id = $1",
            doc
        )
        .await,
        4
    );
    let rows = txns(&pool, doc).await;
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|r| r.2 < Decimal::ZERO && r.4 == date!(2026 - 11 - 10))
    );
    let total: Decimal = rows.iter().map(|r| r.2).sum();
    assert_eq!(total, d("-17"));

    let saved = allocations_of_line(&mut pool.acquire().await.unwrap(), LINE, first.id)
        .await
        .unwrap();
    assert_eq!(saved.len(), 3);
    assert_eq!(saved[0].lot_id, old);
}

/// 不足時不中斷，列出所有不足的項目（同一個批號、商品合併），由呼叫端 rollback。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn shortages_are_collected(pool: PgPool) {
    let actor = user(&pool).await;
    let a = product(&pool, "A001", "goods").await;
    let b = product(&pool, "B001", "goods").await;
    let a_lots = stock(
        &pool,
        actor,
        a,
        "ADJ-1",
        &[("L-01", date!(2026 - 10 - 01), "8", "10")],
    )
    .await;
    stock(
        &pool,
        actor,
        b,
        "ADJ-2",
        &[("M-01", date!(2026 - 10 - 01), "2", "5")],
    )
    .await;

    let doc = document(&pool, "ADJ-3").await;
    let src = source(doc, actor, date!(2026 - 11 - 10));
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![a, b],
            lot_ids: vec![a_lots[0]],
            fifo_product_ids: vec![b],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // 指定批號：第 1 列足夠（剩 3），第 3 列不足
    assert!(
        inv.issue_from_lot(
            &mut tx,
            &src,
            line(0),
            a,
            a_lots[0],
            d("5"),
            TxnType::Adjustment
        )
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        inv.issue_fifo(&mut tx, &src, line(1), b, d("3.5"), TxnType::Adjustment)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        inv.issue_from_lot(
            &mut tx,
            &src,
            line(2),
            a,
            a_lots[0],
            d("6"),
            TxnType::Adjustment
        )
        .await
        .unwrap()
        .is_none()
    );
    let err = inv.finish().unwrap_err();
    let CoreError::StockInsufficient { message, details } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(message, "商品 A001 批號 L-01 庫存不足，尚缺 3 等 2 項");
    assert_eq!(details.items.len(), 2);
    let lot = &details.items[0];
    assert_eq!(lot.product_code, "A001");
    assert_eq!(lot.product_name, "商品 A001");
    assert_eq!(
        (lot.lot_id, lot.lot_no.as_deref()),
        (Some(a_lots[0]), Some("L-01"))
    );
    assert_eq!(
        (
            lot.required.to_string(),
            lot.available.to_string(),
            lot.shortage.to_string()
        ),
        ("11.0000".into(), "8.0000".into(), "3.0000".into())
    );
    let refs = |idx: &[u32]| -> Vec<LineRef> {
        idx.iter()
            .map(|&index| LineRef {
                detail_key: LINE.into(),
                index,
            })
            .collect()
    };
    assert_eq!(lot.lines, refs(&[0, 2]));
    let fifo = &details.items[1];
    assert_eq!((fifo.lot_id, fifo.lot_no.clone()), (None, None));
    assert_eq!(fifo.shortage.to_string(), "1.5000");
    assert_eq!(fifo.lines, refs(&[1]));
    tx.rollback().await.unwrap();

    assert_eq!(remaining(&pool, a_lots[0]).await, d("8"));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM inventory_transactions WHERE ref_doc_id = $1",
            doc
        )
        .await,
        0
    );
}

/// 手動指定批號：批號必須屬於該商品；加回、減少既有批號；作廢時依批號扣除明細加回原批號。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn manual_issue_and_reversal(pool: PgPool) {
    let actor = user(&pool).await;
    let a = product(&pool, "A001", "goods").await;
    let b = product(&pool, "B001", "goods").await;
    let lots = stock(
        &pool,
        actor,
        a,
        "ADJ-1",
        &[
            ("L-01", date!(2026 - 10 - 01), "5", "10"),
            ("L-02", date!(2026 - 10 - 02), "5", "12"),
        ],
    )
    .await;

    let doc = document(&pool, "ADJ-2").await;
    let src = source(doc, actor, date!(2026 - 11 - 10));
    let issue = line(0);
    let gain = line(1);
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![a, b],
            lot_numbers: vec![(a, "l-02".into())],
            lot_ids: vec![lots[0]],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // 批號不屬於商品 B001
    let err = inv
        .issue_from_lot(
            &mut tx,
            &src,
            issue,
            b,
            lots[0],
            d("1"),
            TxnType::Adjustment,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "{err:?}");
    let l2 = inv.lot_by_no(a, "L-02").unwrap().id;
    assert_eq!(l2, lots[1]);
    let allocation = inv
        .issue_from_lot(&mut tx, &src, issue, a, l2, d("4"), TxnType::Adjustment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(allocation.unit_cost_twd, d("12"));
    let added = inv
        .add_to_lot(
            &mut tx,
            &src,
            gain,
            a,
            lots[0],
            d("2"),
            None,
            TxnType::Adjustment,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(added.qty_remaining, d("7"));
    inv.finish().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(remaining(&pool, l2).await, d("1"));
    assert_eq!(remaining(&pool, lots[0]).await, d("7"));

    // 作廢：扣庫存的明細加回、盤盈的明細減回
    let void_src = source(doc, actor, date!(2026 - 11 - 18));
    let mut tx = pool.begin().await.unwrap();
    let allocations = allocations_of_line(&mut tx, LINE, issue.id).await.unwrap();
    assert_eq!(allocations.len(), 1);
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![a],
            lot_ids: allocations
                .iter()
                .map(|x| x.lot_id)
                .chain([lots[0]])
                .collect(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        inv.reverse_allocations(&mut tx, &void_src, issue, &allocations)
            .await
            .unwrap(),
        1
    );
    assert!(
        inv.reduce_lot(
            &mut tx,
            &void_src,
            gain,
            a,
            lots[0],
            d("2"),
            TxnType::VoidReversal
        )
        .await
        .unwrap()
    );
    inv.finish().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(remaining(&pool, l2).await, d("5"));
    assert_eq!(remaining(&pool, lots[0]).await, d("5"));

    let rows = txns(&pool, doc).await;
    let summary: Vec<(&str, Decimal, Decimal)> =
        rows.iter().map(|r| (r.0.as_str(), r.2, r.3)).collect();
    assert_eq!(
        summary,
        [
            ("adjustment", d("-4"), d("12")),
            ("adjustment", d("2"), d("10")),
            ("void_reversal", d("4"), d("12")),
            ("void_reversal", d("-2"), d("10")),
        ]
    );
    assert_eq!(rows[2].4, date!(2026 - 11 - 18));
    // 批號扣除明細保留
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM lot_allocations WHERE source_doc_id = $1",
            doc
        )
        .await,
        1
    );
}

/// 減少既有批號時不足 → 記下不足。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn reduce_lot_reports_shortage(pool: PgPool) {
    let actor = user(&pool).await;
    let a = product(&pool, "A001", "goods").await;
    let lots = stock(
        &pool,
        actor,
        a,
        "ADJ-1",
        &[("L-01", date!(2026 - 10 - 01), "1", "10")],
    )
    .await;
    let doc = document(&pool, "ADJ-2").await;
    let src = source(doc, actor, date!(2026 - 11 - 18));
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![a],
            lot_ids: lots.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        !inv.reduce_lot(
            &mut tx,
            &src,
            line(0),
            a,
            lots[0],
            d("2"),
            TxnType::VoidReversal
        )
        .await
        .unwrap()
    );
    let err = inv.finish().unwrap_err();
    assert!(
        matches!(err, CoreError::StockInsufficient { .. }),
        "{err:?}"
    );
}

/// 刪除批號：被其他單據的有效異動使用時拒絕（`LOT_IN_USE`，列出單號）；其他單據作廢後可以刪除。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn remove_lot_checks_other_documents(pool: PgPool) {
    let actor = user(&pool).await;
    let a = product(&pool, "A001", "goods").await;
    let lots = stock(
        &pool,
        actor,
        a,
        "ADJ-1",
        &[("L-01", date!(2026 - 10 - 01), "5", "10")],
    )
    .await;
    let origin: Id = sqlx::query_scalar("SELECT source_doc_id FROM inventory_lots WHERE id = $1")
        .bind(lots[0])
        .fetch_one(&pool)
        .await
        .unwrap();
    let origin_line: Id =
        sqlx::query_scalar("SELECT source_line_id FROM inventory_lots WHERE id = $1")
            .bind(lots[0])
            .fetch_one(&pool)
            .await
            .unwrap();

    // 另一張單據扣了 1
    let other = document(&pool, "ADJ-2").await;
    let other_line = line(0);
    {
        let src = source(other, actor, date!(2026 - 11 - 02));
        let mut tx = pool.begin().await.unwrap();
        let mut inv = fifo_lock(&mut tx, a).await;
        inv.issue_fifo(&mut tx, &src, other_line, a, d("1"), TxnType::Adjustment)
            .await
            .unwrap();
        inv.finish().unwrap();
        tx.commit().await.unwrap();
    }

    let src = source(origin, actor, date!(2026 - 11 - 18));
    let remove = Line {
        id: origin_line,
        index: 0,
    };
    let lock = LockRequest {
        product_ids: vec![a],
        lot_ids: lots.clone(),
        ..Default::default()
    };
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(&mut tx, &lock).await.unwrap();
    assert!(
        !inv.remove_lot(&mut tx, &src, remove, lots[0])
            .await
            .unwrap()
    );
    let err = inv.finish().unwrap_err();
    let CoreError::LotInUse { message, details } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(message, "批號 L-01 已被其他單據使用，請先作廢 ADJ-2");
    assert_eq!(details.lots[0].product_code, "A001");
    assert_eq!(details.lots[0].documents[0].record_id, other);
    assert_eq!(details.lots[0].documents[0].entity_key, DOC);
    tx.rollback().await.unwrap();

    // 作廢另一張單據後，批號沒有其他有效異動，可以刪除
    {
        let src = source(other, actor, date!(2026 - 11 - 18));
        let mut tx = pool.begin().await.unwrap();
        let allocations = allocations_of_line(&mut tx, LINE, other_line.id)
            .await
            .unwrap();
        let mut inv = Inventory::lock(
            &mut tx,
            &LockRequest {
                product_ids: vec![a],
                lot_ids: allocations.iter().map(|x| x.lot_id).collect(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        inv.reverse_allocations(&mut tx, &src, other_line, &allocations)
            .await
            .unwrap();
        inv.finish().unwrap();
        tx.commit().await.unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(&mut tx, &lock).await.unwrap();
    assert!(
        inv.remove_lot(&mut tx, &src, remove, lots[0])
            .await
            .unwrap()
    );
    assert!(inv.lot(lots[0]).is_none());
    inv.finish().unwrap();
    tx.commit().await.unwrap();

    let (deleted, qty): (bool, Decimal) = sqlx::query_as(
        "SELECT deleted_at IS NOT NULL, qty_remaining FROM inventory_lots WHERE id = $1",
    )
    .bind(lots[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(deleted);
    assert_eq!(qty, Decimal::ZERO);
    let rows = txns(&pool, origin).await;
    assert_eq!(rows.last().unwrap().0, "void_reversal");
    assert_eq!(rows.last().unwrap().2, d("-5"));
}

/// `service` 商品不經過庫存引擎：不建立批號、不扣庫存、不寫入異動。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn service_products_are_skipped(pool: PgPool) {
    let actor = user(&pool).await;
    let s = product(&pool, "S001", "service").await;
    let doc = document(&pool, "ADJ-1").await;
    let src = source(doc, actor, date!(2026 - 11 - 18));
    let mut tx = pool.begin().await.unwrap();
    let mut inv = fifo_lock(&mut tx, s).await;
    assert!(!inv.product(s).unwrap().is_goods);
    assert!(
        inv.receive(
            &mut tx,
            &src,
            line(0),
            &receipt(s, None, "ADJ-1", 0, date!(2026 - 11 - 18), "1", "1"),
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        inv.issue_fifo(&mut tx, &src, line(1), s, d("100"), TxnType::Adjustment)
            .await
            .unwrap()
            .is_empty()
    );
    inv.finish().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM inventory_transactions WHERE ref_doc_id = $1",
            doc
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM inventory_lots WHERE source_doc_id = $1",
            doc
        )
        .await,
        0
    );
}

/// 沒有先鎖定的商品、已刪除的商品、數量不大於 0 → 錯誤。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn invalid_operations_are_rejected(pool: PgPool) {
    let actor = user(&pool).await;
    let a = product(&pool, "A001", "goods").await;
    let gone = product(&pool, "D001", "goods").await;
    sqlx::query("UPDATE products SET deleted_at = now() WHERE id = $1")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();
    let doc = document(&pool, "ADJ-1").await;
    let src = source(doc, actor, date!(2026 - 11 - 18));
    let mut tx = pool.begin().await.unwrap();
    let mut inv = Inventory::lock(
        &mut tx,
        &LockRequest {
            product_ids: vec![gone],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let not_locked = inv
        .receive(
            &mut tx,
            &src,
            line(0),
            &receipt(a, None, "ADJ-1", 0, date!(2026 - 11 - 18), "1", "1"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(not_locked, CoreError::Internal(_)),
        "{not_locked:?}"
    );
    let deleted = inv
        .receive(
            &mut tx,
            &src,
            line(0),
            &receipt(gone, None, "ADJ-1", 0, date!(2026 - 11 - 18), "1", "1"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&deleted, CoreError::Validation(m) if m.contains("已刪除")),
        "{deleted:?}"
    );
    // 已刪除的商品也不能扣庫存
    let err = inv
        .issue_fifo(&mut tx, &src, line(0), gone, d("1"), TxnType::Adjustment)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "{err:?}");
    let mut inv = fifo_lock(&mut tx, a).await;
    let err = inv
        .issue_fifo(&mut tx, &src, line(0), a, d("0"), TxnType::Adjustment)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "{err:?}");
}

/// 兩個 transaction 同時以先進先出扣同一商品：批號以 `FOR UPDATE` 鎖定，後到的等待前一個 commit 後
/// 才讀取數量，不會超扣（`qty_remaining` 不會小於 0）。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn concurrent_issues_do_not_oversell(pool: PgPool) {
    let actor = user(&pool).await;
    let a = product(&pool, "A001", "goods").await;
    let lots = stock(
        &pool,
        actor,
        a,
        "ADJ-0",
        &[
            ("L-01", date!(2026 - 10 - 01), "6", "10"),
            ("L-02", date!(2026 - 10 - 02), "4", "10"),
        ],
    )
    .await;
    let doc1 = document(&pool, "ADJ-1").await;
    let doc2 = document(&pool, "ADJ-2").await;

    // 第一個 transaction 先鎖定並扣 7，暫不 commit
    let mut tx1 = pool.begin().await.unwrap();
    let mut inv1 = fifo_lock(&mut tx1, a).await;
    inv1.issue_fifo(
        &mut tx1,
        &source(doc1, actor, date!(2026 - 11 - 18)),
        line(0),
        a,
        d("7"),
        TxnType::Adjustment,
    )
    .await
    .unwrap();

    // 第二個 transaction 也要扣 7：鎖定批號時等待第一個
    let pool2 = pool.clone();
    let second = tokio::spawn(async move {
        let mut tx2 = pool2.begin().await.unwrap();
        let mut inv2 = fifo_lock(&mut tx2, a).await;
        inv2.issue_fifo(
            &mut tx2,
            &source(doc2, actor, date!(2026 - 11 - 18)),
            line(0),
            a,
            d("7"),
            TxnType::Adjustment,
        )
        .await
        .unwrap();
        let result = inv2.finish();
        match result {
            Ok(()) => tx2.commit().await.unwrap(),
            Err(_) => tx2.rollback().await.unwrap(),
        }
        result
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !second.is_finished(),
        "第二個 transaction 應該在等待批號的鎖"
    );
    inv1.finish().unwrap();
    tx1.commit().await.unwrap();

    let err = second.await.unwrap().unwrap_err();
    let CoreError::StockInsufficient { details, .. } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(details.items[0].available.to_string(), "3.0000");
    assert_eq!(details.items[0].shortage.to_string(), "4.0000");

    assert_eq!(remaining(&pool, lots[0]).await, Decimal::ZERO);
    assert_eq!(remaining(&pool, lots[1]).await, d("3"));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM lot_allocations WHERE source_doc_id = $1",
            doc2
        )
        .await,
        0
    );

    // 同時開始、各扣 2（庫存剩 3）：只有一個成功
    let run = |doc: Id| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let mut inv = fifo_lock(&mut tx, a).await;
            inv.issue_fifo(
                &mut tx,
                &source(doc, actor, date!(2026 - 11 - 18)),
                line(0),
                a,
                d("2"),
                TxnType::Adjustment,
            )
            .await
            .unwrap();
            let result = inv.finish();
            match result {
                Ok(()) => tx.commit().await.unwrap(),
                Err(_) => tx.rollback().await.unwrap(),
            }
            result.is_ok()
        }
    };
    let doc3 = document(&pool, "ADJ-3").await;
    let doc4 = document(&pool, "ADJ-4").await;
    let (r3, r4) = tokio::join!(tokio::spawn(run(doc3)), tokio::spawn(run(doc4)));
    // 後鎖定的那個讀到剩 1，不足
    let ok = [r3.unwrap(), r4.unwrap()];
    assert_eq!(ok.iter().filter(|x| **x).count(), 1, "{ok:?}");
    assert_eq!(remaining(&pool, lots[1]).await, d("1"));
    let negative: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_lots WHERE qty_remaining < 0")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(negative, 0);
}
