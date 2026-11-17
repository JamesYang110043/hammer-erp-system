//! 0011_inventory 的整合測試：庫存資料表、盤點調整單與其資料類別、欄位定義的登記，
//! 以及 `hammer_db::inventory` 的存取函式（docs/architecture.md §7.1、§8.10）。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use hammer_db::inventory::{
    self, APPEND_ONLY_SQLSTATE, LOT_NO_UNIQUE, LOT_SOURCE_LINE_UNIQUE, NewAllocation, NewLot,
    NewTransaction,
};
use hammer_db::sqlx::{self, PgPool, migrate::Migrator};
use hammer_shared::schema::{SelectOption, Validation};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use time::macros::date;
use uuid::Uuid;

/// 0011 之前的最後一個 migration 版本。
const BEFORE_INVENTORY: i64 = 10;

/// `inventory_lots.source_type`、`lot_allocations.source_type` 目前唯一的值。
const LINE: &str = "stock_adjustment_line";
/// `inventory_transactions.ref_doc_type` 目前唯一的值。
const DOC: &str = "stock_adjustment";

fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// 取出資料庫錯誤的 SQLSTATE 與約束名稱。
fn db_error(err: &sqlx::Error) -> (String, Option<String>) {
    let db = err.as_database_error().expect("應為資料庫錯誤");
    (
        db.code().unwrap().into_owned(),
        db.constraint().map(str::to_owned),
    )
}

fn hammer_db_error(err: &hammer_db::DbError) -> (String, Option<String>) {
    match err {
        hammer_db::DbError::Sqlx(e) => db_error(e),
        other => panic!("應為資料庫錯誤：{other:?}"),
    }
}

async fn exec(pool: &PgPool, sql: &str) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(pool)
        .await
        .map(|_| ())
}

/// 新增一個 `goods` 商品，回傳 id。
async fn insert_product(pool: &PgPool, code: &str) -> Uuid {
    let unit = Uuid::now_v7();
    sqlx::query("INSERT INTO units (id, code, name) VALUES ($1, $2, $2)")
        .bind(unit)
        .bind(format!("u-{code}"))
        .execute(pool)
        .await
        .unwrap();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO products (id, code, name, unit_id) VALUES ($1, $2, $2, $3)")
        .bind(id)
        .bind(code)
        .bind(unit)
        .execute(pool)
        .await
        .unwrap();
    id
}

fn new_lot<'a>(product_id: Uuid, lot_no: &'a str, qty: &str, cost: &str) -> NewLot<'a> {
    NewLot {
        id: Uuid::now_v7(),
        product_id,
        lot_no,
        received_date: date!(2026 - 11 - 01),
        source_type: LINE,
        source_doc_id: Uuid::now_v7(),
        source_line_id: Uuid::now_v7(),
        qty: dec(qty),
        unit_cost_twd: dec(cost),
        created_by: None,
    }
}

fn new_txn(
    lot: &NewLot<'_>,
    txn_type: &'static str,
    qty: &str,
    doc: Uuid,
) -> NewTransaction<'static> {
    NewTransaction {
        id: Uuid::now_v7(),
        txn_date: date!(2026 - 11 - 01),
        txn_type,
        product_id: lot.product_id,
        lot_id: lot.id,
        qty_change: dec(qty),
        unit_cost_twd: lot.unit_cost_twd,
        ref_doc_type: DOC,
        ref_doc_id: doc,
        ref_line_id: Uuid::now_v7(),
        created_by: None,
    }
}

// ---------------------------------------------------------------------------
// migration
// ---------------------------------------------------------------------------

/// 在已有 M3 資料的資料庫上套用 0011：既有資料保留，盤點調整單登記為系統資料類別。
#[sqlx::test(migrations = false)]
async fn applies_on_existing_m3_database(pool: PgPool) {
    let before = Migrator::with_migrations(
        hammer_db::MIGRATOR
            .iter()
            .filter(|m| m.version <= BEFORE_INVENTORY)
            .cloned()
            .collect(),
    );
    before.run(&pool).await.unwrap();
    let product = insert_product(&pool, "P-001").await;

    hammer_db::run_migrations(&pool).await.unwrap();
    // 重複執行不會出錯
    hammer_db::run_migrations(&pool).await.unwrap();

    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM entity_definitions WHERE key LIKE 'stock_adjustment%' ORDER BY key",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(keys, ["stock_adjustment", "stock_adjustment_line"]);
    let products: i64 = sqlx::query_scalar("SELECT count(*) FROM products WHERE id = $1")
        .bind(product)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(products, 1);

    // 新的資料表可以使用
    let lot = new_lot(product, "L1", "3", "10");
    inventory::insert_lot(&pool, &lot).await.unwrap();
    assert_eq!(
        inventory::stock_balance(&pool, product).await.unwrap(),
        (dec("3"), dec("30"))
    );
}

// ---------------------------------------------------------------------------
// 資料類別與欄位定義
// ---------------------------------------------------------------------------

/// `stock_adjustment`、`stock_adjustment_line` 與 §8.10 的表一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn stock_adjustment_entities_are_registered(pool: PgPool) {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        String,
        String,
        bool,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i32,
        bool,
    )> = sqlx::query_as(
        "SELECT e.key, e.label, e.kind, e.is_system, e.storage, e.table_name, p.key,
                e.numbering_pattern, e.title_template, e.icon, e.sort_order, e.is_active
         FROM entity_definitions e
         LEFT JOIN entity_definitions p ON p.id = e.parent_entity_id
         WHERE e.key LIKE 'stock_adjustment%'
         ORDER BY e.id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let rows: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r.0.as_str(),
                r.1.as_str(),
                r.2.as_str(),
                r.3,
                r.4.as_str(),
                r.5.as_deref(),
                r.6.as_deref(),
                r.7.as_deref(),
                r.8.as_deref(),
                r.9.as_deref(),
                r.10,
                r.11,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "stock_adjustment",
                "盤點調整單",
                "document",
                true,
                "table",
                Some("stock_adjustments"),
                None,
                Some("ADJ{yyyyMM}-{0000}"),
                Some("{record_no}"),
                None,
                100,
                true,
            ),
            (
                "stock_adjustment_line",
                "盤點調整明細",
                "detail",
                true,
                "table",
                Some("stock_adjustment_lines"),
                Some("stock_adjustment"),
                None,
                Some("{product_id.code} {product_id.name} {lot_no}"),
                None,
                0,
                true,
            ),
        ]
    );
}

/// 一個欄位定義的預期值：`(field_key, label, data_type, is_required, is_readonly, default_value,
/// validation, options 的值, decimal_places, 關聯目標)`。
type FieldSpec = (
    &'static str,
    &'static str,
    &'static str,
    bool,
    bool,
    Option<Value>,
    Option<Value>,
    Option<Vec<&'static str>>,
    Option<i16>,
    Option<&'static str>,
);

fn field_specs(entity: &str) -> Vec<FieldSpec> {
    match entity {
        "stock_adjustment" => vec![
            (
                "adjustment_date",
                "調整日期",
                "date",
                true,
                false,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "adjustment_type",
                "類型",
                "select",
                true,
                false,
                Some(json!("count")),
                None,
                Some(vec!["opening", "count", "adjustment"]),
                None,
                None,
            ),
            (
                "status",
                "狀態",
                "select",
                false,
                true,
                None,
                None,
                Some(vec!["draft", "posted", "void"]),
                None,
                None,
            ),
            (
                "notes",
                "備註",
                "long_text",
                false,
                false,
                None,
                None,
                None,
                None,
                None,
            ),
        ],
        "stock_adjustment_line" => vec![
            (
                "product_id",
                "商品",
                "relation",
                true,
                false,
                None,
                None,
                None,
                None,
                Some("product"),
            ),
            (
                "line_type",
                "調整方式",
                "select",
                true,
                false,
                Some(json!("gain_new")),
                None,
                Some(vec!["gain_new", "gain_existing", "loss"]),
                None,
                None,
            ),
            (
                "lot_no",
                "批號",
                "text",
                false,
                false,
                None,
                Some(json!({ "max_length": 50 })),
                None,
                None,
                None,
            ),
            (
                "qty",
                "數量",
                "decimal",
                true,
                false,
                None,
                Some(json!({ "min": "0" })),
                None,
                Some(4),
                None,
            ),
            (
                "unit_cost_twd",
                "單位成本",
                "money",
                false,
                false,
                None,
                Some(json!({ "min": "0" })),
                None,
                Some(4),
                None,
            ),
            (
                "reason", "原因", "text", false, false, None, None, None, None, None,
            ),
        ],
        other => panic!("未知的資料類別：{other}"),
    }
}

#[derive(Debug, sqlx::FromRow)]
struct FieldRow {
    field_key: String,
    label: String,
    data_type: String,
    is_system: bool,
    storage: String,
    column_name: Option<String>,
    is_required: bool,
    is_visible: bool,
    is_readonly: bool,
    is_searchable: bool,
    is_unique: bool,
    default_value: Option<Value>,
    validation: Option<Value>,
    options: Option<Value>,
    decimal_places: Option<i16>,
    relation_key: Option<String>,
    ui_widget: Option<String>,
    sort_order: i32,
    is_active: bool,
}

/// 系統欄位的定義與 §8.10 的表一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn system_fields_match_spec(pool: PgPool) {
    for entity in ["stock_adjustment", "stock_adjustment_line"] {
        let rows: Vec<FieldRow> = sqlx::query_as(
            "SELECT f.field_key, f.label, f.data_type, f.is_system, f.storage, f.column_name,
                    f.is_required, f.is_visible, f.is_readonly, f.is_searchable, f.is_unique,
                    f.default_value, f.validation, f.options, f.decimal_places,
                    r.key AS relation_key, f.ui_widget, f.sort_order, f.is_active
             FROM field_definitions f
             JOIN entity_definitions e ON e.id = f.entity_id
             LEFT JOIN entity_definitions r ON r.id = f.relation_entity_id
             WHERE e.key = $1
             ORDER BY f.sort_order",
        )
        .bind(entity)
        .fetch_all(&pool)
        .await
        .unwrap();
        let specs = field_specs(entity);
        let keys: Vec<&str> = rows.iter().map(|r| r.field_key.as_str()).collect();
        let spec_keys: Vec<&str> = specs.iter().map(|s| s.0).collect();
        assert_eq!(keys, spec_keys, "{entity} 的欄位與順序");

        for (i, (row, spec)) in rows.iter().zip(&specs).enumerate() {
            let at = format!("{entity}.{}", spec.0);
            assert_eq!(row.label, spec.1, "{at}");
            assert_eq!(row.data_type, spec.2, "{at}");
            assert!(row.is_system, "{at}");
            assert_eq!(row.storage, "column", "{at}");
            assert_eq!(row.column_name.as_deref(), Some(spec.0), "{at}");
            assert_eq!(row.is_required, spec.3, "{at} is_required");
            assert_eq!(row.is_readonly, spec.4, "{at} is_readonly");
            assert!(row.is_visible && row.is_active, "{at}");
            assert!(!row.is_unique && !row.is_searchable, "{at}");
            assert_eq!(row.default_value, spec.5, "{at} default_value");
            assert_eq!(row.validation, spec.6, "{at} validation");
            let options: Option<Vec<SelectOption>> = row
                .options
                .clone()
                .map(|v| serde_json::from_value(v).unwrap());
            let values: Option<Vec<&str>> = options
                .as_ref()
                .map(|o| o.iter().map(|o| o.value.as_str()).collect());
            assert_eq!(values, spec.7, "{at} options");
            assert_eq!(row.decimal_places, spec.8, "{at}");
            assert_eq!(row.relation_key.as_deref(), spec.9, "{at}");
            assert_eq!(row.ui_widget, None, "{at}");
            assert_eq!(row.sort_order, (i as i32 + 1) * 10, "{at} sort_order");
            if let Some(v) = &row.validation {
                serde_json::from_value::<Validation>(v.clone()).unwrap();
            }
        }
    }
}

/// 實體欄位的順序：id → 不是欄位的單號或父資料欄位 → 系統欄位 → 不是欄位的過帳資訊或 lot_id →
/// custom_data、formula_errors、共通欄位。數值欄位為 NUMERIC(18,4)。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn table_columns(pool: PgPool) {
    const TAIL: &[&str] = &[
        "custom_data",
        "formula_errors",
        "created_at",
        "created_by",
        "updated_at",
        "updated_by",
        "deleted_at",
        "version",
    ];
    for (table, head, fields, after) in [
        (
            "stock_adjustments",
            &["id", "record_no"][..],
            &["adjustment_date", "adjustment_type", "status", "notes"][..],
            &["posted_at", "posted_by", "voided_at", "voided_by"][..],
        ),
        (
            "stock_adjustment_lines",
            &["id", "stock_adjustment_id", "sort_order"][..],
            &[
                "product_id",
                "line_type",
                "lot_no",
                "qty",
                "unit_cost_twd",
                "reason",
            ][..],
            &["lot_id"][..],
        ),
    ] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name::text FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = $1
             ORDER BY ordinal_position",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .unwrap();
        let expected: Vec<&str> = [head, fields, after, TAIL].concat();
        assert_eq!(columns, expected, "{table}");
    }

    let numeric: Vec<(String, String, i32, i32)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text, numeric_precision::int, numeric_scale::int
         FROM information_schema.columns
         WHERE table_schema = current_schema() AND data_type = 'numeric'
           AND table_name IN ('stock_adjustment_lines', 'inventory_lots', 'lot_allocations',
                              'inventory_transactions')
         ORDER BY table_name, column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(numeric.len(), 9, "{numeric:?}");
    for (table, column, precision, scale) in numeric {
        assert_eq!((precision, scale), (18, 4), "{table}.{column}");
    }
}

/// 索引與 trigger。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn indexes_and_triggers(pool: PgPool) {
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes WHERE schemaname = current_schema()",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for name in [
        "uq_stock_adjustments_record_no",
        "idx_stock_adjustments_adjustment_date",
        "idx_stock_adjustments_status",
        "idx_stock_adjustments_custom_data",
        "idx_stock_adjustment_lines_stock_adjustment_id",
        "idx_stock_adjustment_lines_product_id",
        "idx_stock_adjustment_lines_lot_id",
        "idx_stock_adjustment_lines_custom_data",
        "uq_inventory_lots_product_lot_no",
        "idx_inventory_lots_fifo",
        "idx_inventory_lots_source_doc_id",
        "inventory_lots_source_line_key",
        "idx_lot_allocations_source_line",
        "idx_lot_allocations_lot_id",
        "idx_inventory_transactions_product_date",
        "idx_inventory_transactions_lot_ref",
        "idx_inventory_transactions_ref",
    ] {
        assert!(indexes.iter().any(|i| i == name), "缺少索引 {name}");
    }

    for table in [
        "stock_adjustments",
        "stock_adjustment_lines",
        "inventory_lots",
    ] {
        let has: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid
                 WHERE t.tgrelid = $1::regclass AND p.proname = 'set_updated_at'
             )",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(has, "{table} 沒有 set_updated_at trigger");
    }
}

// ---------------------------------------------------------------------------
// 限制
// ---------------------------------------------------------------------------

/// `inventory_transactions`、`lot_allocations` 只能新增：UPDATE、DELETE、TRUNCATE 都被拒絕。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn ledger_tables_are_append_only(pool: PgPool) {
    let product = insert_product(&pool, "P").await;
    let lot = new_lot(product, "L1", "5", "10");
    inventory::insert_lot(&pool, &lot).await.unwrap();
    let doc = Uuid::now_v7();
    inventory::insert_transaction(&pool, &new_txn(&lot, "adjustment", "5", doc))
        .await
        .unwrap();
    inventory::insert_allocation(
        &pool,
        &NewAllocation {
            id: Uuid::now_v7(),
            source_type: LINE,
            source_doc_id: doc,
            source_line_id: Uuid::now_v7(),
            lot_id: lot.id,
            qty: dec("1"),
            unit_cost_twd: dec("10"),
            created_by: None,
        },
    )
    .await
    .unwrap();

    for sql in [
        "UPDATE inventory_transactions SET qty_change = 1",
        "DELETE FROM inventory_transactions",
        "TRUNCATE inventory_transactions",
        "UPDATE lot_allocations SET qty = 2",
        "DELETE FROM lot_allocations",
        "TRUNCATE lot_allocations",
    ] {
        let err = exec(&pool, sql).await.unwrap_err();
        assert_eq!(db_error(&err).0, APPEND_ONLY_SQLSTATE, "{sql}");
    }
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM inventory_transactions),
                (SELECT count(*) FROM lot_allocations)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1));
}

/// 數量與列舉值的 CHECK。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn inventory_checks(pool: PgPool) {
    let product = insert_product(&pool, "P").await;
    let lot = new_lot(product, "L1", "5", "10");
    inventory::insert_lot(&pool, &lot).await.unwrap();

    // qty_remaining >= 0
    let err = inventory::update_lot_qty_remaining(&pool, lot.id, dec("-0.0001"), None)
        .await
        .unwrap_err();
    assert_eq!(hammer_db_error(&err).0, "23514");
    // qty_remaining 可以大於 qty_in（盤盈加回）、可以為 0
    assert!(
        inventory::update_lot_qty_remaining(&pool, lot.id, dec("8"), None)
            .await
            .unwrap()
    );
    assert!(
        inventory::update_lot_qty_remaining(&pool, lot.id, Decimal::ZERO, None)
            .await
            .unwrap()
    );

    // 批號：qty_in > 0、成本 >= 0、source_type、批號長度
    for bad in [
        NewLot {
            qty: Decimal::ZERO,
            ..new_lot(product, "L2", "1", "1")
        },
        NewLot {
            unit_cost_twd: dec("-1"),
            ..new_lot(product, "L2", "1", "1")
        },
        NewLot {
            source_type: "goods_receipt_line",
            ..new_lot(product, "L2", "1", "1")
        },
        new_lot(product, "", "1", "1"),
        new_lot(product, &"x".repeat(51), "1", "1"),
    ] {
        let err = inventory::insert_lot(&pool, &bad).await.unwrap_err();
        assert_eq!(hammer_db_error(&err).0, "23514", "{bad:?}");
    }
    // 軟刪除的批號 qty_remaining 必須為 0
    let err = exec(
        &pool,
        "UPDATE inventory_lots SET deleted_at = now(), qty_remaining = 1",
    )
    .await
    .unwrap_err();
    assert_eq!(
        db_error(&err),
        (
            "23514".to_owned(),
            Some("inventory_lots_deleted_empty_check".to_owned())
        )
    );

    // 異動：qty_change <> 0、txn_type、ref_doc_type
    let doc = Uuid::now_v7();
    for bad in [
        new_txn(&lot, "adjustment", "0", doc),
        new_txn(&lot, "transfer", "1", doc),
        NewTransaction {
            ref_doc_type: "shipment",
            ..new_txn(&lot, "adjustment", "1", doc)
        },
    ] {
        let err = inventory::insert_transaction(&pool, &bad)
            .await
            .unwrap_err();
        assert_eq!(hammer_db_error(&err).0, "23514", "{bad:?}");
    }
    for txn_type in [
        "receipt",
        "shipment",
        "sales_return",
        "purchase_return",
        "adjustment",
        "void_reversal",
    ] {
        inventory::insert_transaction(&pool, &new_txn(&lot, txn_type, "-1", doc))
            .await
            .unwrap();
    }

    // 扣除明細：qty > 0
    let err = inventory::insert_allocation(
        &pool,
        &NewAllocation {
            id: Uuid::now_v7(),
            source_type: LINE,
            source_doc_id: doc,
            source_line_id: Uuid::now_v7(),
            lot_id: lot.id,
            qty: Decimal::ZERO,
            unit_cost_twd: dec("10"),
            created_by: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(hammer_db_error(&err).0, "23514");
}

/// 同一商品未刪除的批號不分大小寫不可重複；刪除後可再使用；一列明細最多一個批號。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn lot_no_is_unique_per_product(pool: PgPool) {
    let a = insert_product(&pool, "A").await;
    let b = insert_product(&pool, "B").await;
    let first = new_lot(a, "LOT-1", "1", "1");
    inventory::insert_lot(&pool, &first).await.unwrap();

    let err = inventory::insert_lot(&pool, &new_lot(a, "lot-1", "1", "1"))
        .await
        .unwrap_err();
    assert_eq!(
        hammer_db_error(&err),
        ("23505".to_owned(), Some(LOT_NO_UNIQUE.to_owned()))
    );
    // 其他商品可以使用同一個批號
    inventory::insert_lot(&pool, &new_lot(b, "LOT-1", "1", "1"))
        .await
        .unwrap();

    let err = inventory::insert_lot(
        &pool,
        &NewLot {
            source_line_id: first.source_line_id,
            ..new_lot(a, "LOT-2", "1", "1")
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        hammer_db_error(&err),
        ("23505".to_owned(), Some(LOT_SOURCE_LINE_UNIQUE.to_owned()))
    );

    assert!(
        inventory::soft_delete_lot(&pool, first.id, None)
            .await
            .unwrap()
    );
    assert!(
        !inventory::soft_delete_lot(&pool, first.id, None)
            .await
            .unwrap()
    );
    inventory::insert_lot(&pool, &new_lot(a, "lot-1", "1", "1"))
        .await
        .unwrap();
}

/// 盤點調整單：狀態與過帳、作廢欄位的一致性；單號唯一（含已刪除）；明細的 CHECK。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn stock_adjustment_checks(pool: PgPool) {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, username, display_name, password_hash, role_id)
         SELECT gen_random_uuid(), 'u', 'u', 'x', id FROM roles WHERE code = 'admin'
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let insert = |no: &'static str, extra_cols: &'static str, extra_vals: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO stock_adjustments (id, record_no, adjustment_date, created_by{extra_cols})
                 VALUES ($1, $2, '2026-11-01', $3{extra_vals})"
            )))
            .bind(Uuid::now_v7())
            .bind(no)
            .bind(user)
            .execute(&pool)
            .await
            .map(|_| ())
        }
    };
    // $3 為使用者（created_by，也用於過帳、作廢人員）
    insert("ADJ-1", "", "").await.unwrap();
    let (kind, status): (String, String) =
        sqlx::query_as("SELECT adjustment_type, status FROM stock_adjustments")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((kind.as_str(), status.as_str()), ("count", "draft"));

    // 已過帳、已作廢
    insert(
        "ADJ-2",
        ", status, posted_at, posted_by",
        ", 'posted', now(), $3",
    )
    .await
    .unwrap();
    insert(
        "ADJ-3",
        ", status, posted_at, posted_by, voided_at, voided_by",
        ", 'void', now(), $3, now(), $3",
    )
    .await
    .unwrap();

    for (cols, vals) in [
        (", adjustment_type", ", 'transfer'"),
        (", status", ", 'closed'"),
        // draft 不可有過帳資訊
        (", posted_at, posted_by", ", now(), $3"),
        // posted 必須有過帳資訊
        (", status", ", 'posted'"),
        // 過帳時間與人員同時有值
        (", status, posted_at", ", 'posted', now()"),
        // posted 不可有作廢資訊
        (
            ", status, posted_at, posted_by, voided_at, voided_by",
            ", 'posted', now(), $3, now(), $3",
        ),
        // void 必須有作廢時間
        (", status, posted_at, posted_by", ", 'void', now(), $3"),
        (", custom_data", ", '[]'"),
    ] {
        let err = insert("ADJ-X", cols, vals).await.unwrap_err();
        assert_eq!(db_error(&err).0, "23514", "{cols} = {vals}");
    }

    // 單號唯一，已刪除的資料也計入
    exec(
        &pool,
        "UPDATE stock_adjustments SET deleted_at = now() WHERE record_no = 'ADJ-1'",
    )
    .await
    .unwrap();
    let err = insert("ADJ-1", "", "").await.unwrap_err();
    assert_eq!(
        db_error(&err),
        (
            "23505".to_owned(),
            Some("uq_stock_adjustments_record_no".to_owned())
        )
    );

    // 明細
    let product = insert_product(&pool, "P").await;
    let doc: Uuid =
        sqlx::query_scalar("SELECT id FROM stock_adjustments WHERE record_no = 'ADJ-2'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let insert_line = |cols: &'static str, vals: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO stock_adjustment_lines (id, stock_adjustment_id, product_id{cols})
                 VALUES ($1, $2, $3{vals})"
            )))
            .bind(Uuid::now_v7())
            .bind(doc)
            .bind(product)
            .execute(&pool)
            .await
            .map(|_| ())
        }
    };
    insert_line(", qty, unit_cost_twd", ", 1, 10")
        .await
        .unwrap();
    let line_type: String = sqlx::query_scalar("SELECT line_type FROM stock_adjustment_lines")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(line_type, "gain_new");
    for (cols, vals) in [
        (", qty", ", 0"),
        (", qty", ", -1"),
        (", qty, unit_cost_twd", ", 1, -0.0001"),
        (", qty, line_type", ", 1, 'transfer'"),
        (", qty, formula_errors", ", 1, '\"x\"'"),
    ] {
        let err = insert_line(cols, vals).await.unwrap_err();
        assert_eq!(db_error(&err).0, "23514", "{cols} = {vals}");
    }
    let err = insert_line("", "").await.unwrap_err();
    assert_eq!(db_error(&err).0, "23502", "qty 必填");
}

// ---------------------------------------------------------------------------
// hammer_db::inventory
// ---------------------------------------------------------------------------

/// 批號的查詢、鎖定、數量更新、軟刪除，以及商品庫存。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn lot_access(pool: PgPool) {
    let a = insert_product(&pool, "A").await;
    let b = insert_product(&pool, "B").await;
    let a1 = new_lot(a, "A-1", "5", "10");
    let a2 = new_lot(a, "A-2", "2", "12.5");
    let b1 = new_lot(b, "B-1", "1", "3");
    for lot in [&a1, &a2, &b1] {
        inventory::insert_lot(&pool, lot).await.unwrap();
    }

    let found = inventory::find_lot(&pool, a1.id).await.unwrap().unwrap();
    assert_eq!(found.lot_no, "A-1");
    assert_eq!(found.qty_in, dec("5"));
    assert_eq!(found.qty_remaining, dec("5"));
    assert_eq!(found.unit_cost_twd, dec("10"));
    assert_eq!(found.received_date, date!(2026 - 11 - 01));
    assert_eq!(found.version, 1);
    assert_eq!(
        inventory::find_lot_by_no(&pool, a, "a-2")
            .await
            .unwrap()
            .map(|l| l.id),
        Some(a2.id)
    );
    assert_eq!(
        inventory::find_lot_by_no(&pool, b, "A-2").await.unwrap(),
        None
    );

    assert_eq!(
        inventory::stock_balance(&pool, a).await.unwrap(),
        (dec("7"), dec("75"))
    );
    let none = Uuid::now_v7();
    assert_eq!(
        inventory::stock_balance(&pool, none).await.unwrap(),
        (Decimal::ZERO, Decimal::ZERO)
    );

    // 數量歸零的批號不在先進先出的範圍，但指定時仍會鎖定
    inventory::update_lot_qty_remaining(&pool, a2.id, Decimal::ZERO, None)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let locked = inventory::lock_lots(&mut *tx, &[b1.id], &[a])
        .await
        .unwrap();
    let ids: Vec<Uuid> = locked.iter().map(|l| l.id).collect();
    // 依 product_id, id 排序
    let mut expected = [(a, a1.id), (b, b1.id)];
    expected.sort();
    assert_eq!(ids, expected.map(|(_, id)| id));
    let locked = inventory::lock_lots(&mut *tx, &[a2.id], &[]).await.unwrap();
    assert_eq!(locked.len(), 1);
    assert_eq!(locked[0].version, 2);
    tx.commit().await.unwrap();

    // 軟刪除後視為不存在，也不計入庫存
    inventory::soft_delete_lot(&pool, a1.id, None)
        .await
        .unwrap();
    assert_eq!(inventory::find_lot(&pool, a1.id).await.unwrap(), None);
    assert!(
        !inventory::update_lot_qty_remaining(&pool, a1.id, dec("1"), None)
            .await
            .unwrap()
    );
    assert!(
        inventory::lock_lots(&pool, &[a1.id], &[a])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        inventory::stock_balance(&pool, a).await.unwrap(),
        (Decimal::ZERO, Decimal::ZERO)
    );
}

/// 扣除明細依明細查詢；批號是否被其他單據的有效異動使用。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn allocations_and_transactions(pool: PgPool) {
    let product = insert_product(&pool, "P").await;
    let lot = new_lot(product, "L", "10", "4");
    inventory::insert_lot(&pool, &lot).await.unwrap();
    let source = lot.source_doc_id;

    let line = Uuid::now_v7();
    let other_doc = Uuid::now_v7();
    for qty in ["1", "2"] {
        inventory::insert_allocation(
            &pool,
            &NewAllocation {
                id: Uuid::now_v7(),
                source_type: LINE,
                source_doc_id: other_doc,
                source_line_id: line,
                lot_id: lot.id,
                qty: dec(qty),
                unit_cost_twd: dec("4"),
                created_by: None,
            },
        )
        .await
        .unwrap();
    }
    let rows = inventory::list_allocations_by_line(&pool, LINE, line)
        .await
        .unwrap();
    let qty: Vec<Decimal> = rows.iter().map(|r| r.qty).collect();
    assert_eq!(qty, [dec("1"), dec("2")]);
    assert!(
        rows.iter()
            .all(|r| r.lot_id == lot.id && r.source_doc_id == other_doc)
    );
    assert!(
        inventory::list_allocations_by_line(&pool, LINE, Uuid::now_v7())
            .await
            .unwrap()
            .is_empty()
    );

    // 只有產生批號的單據自己的異動 → 沒有被其他單據使用
    inventory::insert_transaction(&pool, &new_txn(&lot, "adjustment", "10", source))
        .await
        .unwrap();
    assert!(
        !inventory::lot_used_by_other_documents(&pool, lot.id, DOC, source)
            .await
            .unwrap()
    );
    // 其他單據扣了這個批號 → 被使用
    inventory::insert_transaction(&pool, &new_txn(&lot, "adjustment", "-3", other_doc))
        .await
        .unwrap();
    assert!(
        inventory::lot_used_by_other_documents(&pool, lot.id, DOC, source)
            .await
            .unwrap()
    );
    // 其他單據作廢（有 void_reversal）→ 不再算被使用
    inventory::insert_transaction(&pool, &new_txn(&lot, "void_reversal", "3", other_doc))
        .await
        .unwrap();
    assert!(
        !inventory::lot_used_by_other_documents(&pool, lot.id, DOC, source)
            .await
            .unwrap()
    );
}
