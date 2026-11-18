//! 0012_purchasing 的整合測試：採購單、進貨單、進貨退回的資料表與資料類別、欄位定義的登記，
//! 庫存來源類型的擴充，以及 `hammer_db::purchasing` 的存取函式（docs/architecture.md §8.8）。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use hammer_db::inventory::{self, NewLot, NewTransaction};
use hammer_db::purchasing;
use hammer_db::sqlx::{self, PgPool, migrate::Migrator};
use hammer_shared::schema::{SelectOption, Validation};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use time::macros::date;
use uuid::Uuid;

/// 0012 之前的最後一個 migration 版本。
const BEFORE_PURCHASING: i64 = 11;

fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn sqlstate(err: &sqlx::Error) -> (String, Option<String>) {
    let db = err.as_database_error().expect("應為資料庫錯誤");
    (
        db.code().unwrap().into_owned(),
        db.constraint().map(str::to_owned),
    )
}

async fn exec(pool: &PgPool, sql: &str) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(pool)
        .await
        .map(|_| ())
}

/// 廠商、商品、使用者，回傳 `(supplier_id, product_id, user_id)`。
async fn seed(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let supplier = Uuid::now_v7();
    sqlx::query("INSERT INTO suppliers (id, code, name) VALUES ($1, 'S1', '廠商')")
        .bind(supplier)
        .execute(pool)
        .await
        .unwrap();
    let unit = Uuid::now_v7();
    sqlx::query("INSERT INTO units (id, code, name) VALUES ($1, 'pcs', '個')")
        .bind(unit)
        .execute(pool)
        .await
        .unwrap();
    let product = Uuid::now_v7();
    sqlx::query("INSERT INTO products (id, code, name, unit_id) VALUES ($1, 'A001', 'A', $2)")
        .bind(product)
        .bind(unit)
        .execute(pool)
        .await
        .unwrap();
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, username, display_name, password_hash, role_id)
         SELECT $1, 'u', 'u', 'x', id FROM roles WHERE code = 'editor' RETURNING id",
    )
    .bind(Uuid::now_v7())
    .fetch_one(pool)
    .await
    .unwrap();
    (supplier, product, user)
}

/// 新增一張進貨單（草稿）與一列明細，回傳 `(goods_receipt_id, line_id)`。
async fn insert_receipt(pool: &PgPool, supplier: Uuid, product: Uuid, no: &str) -> (Uuid, Uuid) {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO goods_receipts (id, record_no, supplier_id, receipt_date, currency_code,
                                     exchange_rate, tax_type)
         VALUES ($1, $2, $3, '2026-11-10', 'USD', 32.15, 'zero_rated')",
    )
    .bind(id)
    .bind(no)
    .bind(supplier)
    .execute(pool)
    .await
    .unwrap();
    let line = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO goods_receipt_lines (id, goods_receipt_id, product_id, qty, unit_price, lot_no)
         VALUES ($1, $2, $3, 10, 12.345, 'L-01')",
    )
    .bind(line)
    .bind(id)
    .bind(product)
    .execute(pool)
    .await
    .unwrap();
    (id, line)
}

// ---------------------------------------------------------------------------
// migration
// ---------------------------------------------------------------------------

/// 在 M4a 的資料庫上套用 0012：既有資料保留，進貨單據登記為系統資料類別。
#[sqlx::test(migrations = false)]
async fn applies_on_existing_m4a_database(pool: PgPool) {
    let before = Migrator::with_migrations(
        hammer_db::MIGRATOR
            .iter()
            .filter(|m| m.version <= BEFORE_PURCHASING)
            .cloned()
            .collect(),
    );
    before.run(&pool).await.unwrap();
    let (supplier, product, _) = seed(&pool).await;

    hammer_db::run_migrations(&pool).await.unwrap();
    hammer_db::run_migrations(&pool).await.unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM entity_definitions
         WHERE key IN ('purchase_order', 'purchase_order_line', 'goods_receipt',
                       'goods_receipt_line', 'purchase_return', 'purchase_return_line')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 6);
    insert_receipt(&pool, supplier, product, "GR-1").await;
}

// ---------------------------------------------------------------------------
// 資料類別與欄位定義
// ---------------------------------------------------------------------------

/// 六個資料類別與 §8.8「資料類別」的表一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn entities_are_registered(pool: PgPool) {
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
         WHERE e.key LIKE 'purchase%' OR e.key LIKE 'goods_receipt%'
         ORDER BY e.id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let doc_title = "{record_no} {supplier_id.name}";
    let line_title = "{product_id.code} {product_id.name} {lot_no}";
    let expected = [
        (
            "purchase_order",
            "採購單",
            "document",
            "purchase_orders",
            None,
            Some("PO{yyyyMM}-{0000}"),
            doc_title,
            110,
        ),
        (
            "purchase_order_line",
            "採購明細",
            "detail",
            "purchase_order_lines",
            Some("purchase_order"),
            None,
            "{product_id.code} {product_id.name}",
            0,
        ),
        (
            "goods_receipt",
            "進貨單",
            "document",
            "goods_receipts",
            None,
            Some("GR{yyyyMM}-{0000}"),
            doc_title,
            120,
        ),
        (
            "goods_receipt_line",
            "進貨明細",
            "detail",
            "goods_receipt_lines",
            Some("goods_receipt"),
            None,
            line_title,
            0,
        ),
        (
            "purchase_return",
            "進貨退回",
            "document",
            "purchase_returns",
            None,
            Some("PR{yyyyMM}-{0000}"),
            doc_title,
            130,
        ),
        (
            "purchase_return_line",
            "進貨退回明細",
            "detail",
            "purchase_return_lines",
            Some("purchase_return"),
            None,
            line_title,
            0,
        ),
    ];
    assert_eq!(rows.len(), expected.len());
    for (row, (key, label, kind, table, parent, pattern, title, sort)) in rows.iter().zip(expected)
    {
        assert_eq!(row.0, key);
        assert_eq!(row.1, label, "{key}");
        assert_eq!(row.2, kind, "{key}");
        assert!(row.3 && row.11, "{key}");
        assert_eq!(row.4, "table", "{key}");
        assert_eq!(row.5.as_deref(), Some(table), "{key}");
        assert_eq!(row.6.as_deref(), parent, "{key}");
        assert_eq!(row.7.as_deref(), pattern, "{key}");
        assert_eq!(row.8.as_deref(), Some(title), "{key}");
        assert_eq!(row.9, None, "{key}");
        assert_eq!(row.10, sort, "{key}");
    }
}

/// 欄位定義的預期值：`(field_key, label, data_type, 必填, 唯讀, 小數, 關聯目標, ui_widget, 顯示)`。
type Spec = (
    &'static str,
    &'static str,
    &'static str,
    bool,
    bool,
    Option<i16>,
    Option<&'static str>,
    Option<&'static str>,
    bool,
);

const TAX: &[&str] = &["taxable", "zero_rated", "exempt"];
const STATUS: &[&str] = &["draft", "posted", "void"];
const PO_STATUS: &[&str] = &["draft", "confirmed", "partially_received", "closed", "void"];

fn specs(entity: &str) -> Vec<Spec> {
    let amounts: Vec<Spec> = vec![
        (
            "subtotal",
            "未稅金額",
            "money",
            false,
            true,
            Some(4),
            None,
            None,
            true,
        ),
        (
            "tax_amount",
            "稅額",
            "money",
            false,
            true,
            Some(4),
            None,
            None,
            true,
        ),
        (
            "total",
            "總計",
            "money",
            false,
            true,
            Some(4),
            None,
            None,
            true,
        ),
        (
            "total_twd",
            "台幣總計",
            "money",
            false,
            true,
            Some(2),
            None,
            None,
            true,
        ),
    ];
    let notes: Spec = (
        "notes",
        "備註",
        "long_text",
        false,
        false,
        None,
        None,
        None,
        true,
    );
    let supplier: Spec = (
        "supplier_id",
        "廠商",
        "relation",
        true,
        false,
        None,
        Some("supplier"),
        None,
        true,
    );
    let product: Spec = (
        "product_id",
        "商品",
        "relation",
        true,
        false,
        None,
        Some("product"),
        None,
        true,
    );
    let qty: Spec = (
        "qty",
        "數量",
        "decimal",
        true,
        false,
        Some(4),
        None,
        None,
        true,
    );
    let price: Spec = (
        "unit_price",
        "單價",
        "money",
        true,
        false,
        Some(4),
        None,
        None,
        true,
    );
    let amount: Spec = (
        "amount",
        "金額",
        "money",
        false,
        true,
        Some(4),
        None,
        None,
        true,
    );
    let cost: Spec = (
        "unit_cost_twd",
        "台幣單位成本",
        "money",
        false,
        true,
        Some(4),
        None,
        None,
        true,
    );
    let currency: Spec = (
        "currency_code",
        "幣別",
        "text",
        true,
        false,
        None,
        None,
        Some("currency"),
        true,
    );
    let rate: Spec = (
        "exchange_rate",
        "匯率",
        "decimal",
        false,
        false,
        Some(6),
        None,
        None,
        true,
    );
    let tax: Spec = (
        "tax_type", "稅別", "select", true, false, None, None, None, true,
    );
    let status: Spec = (
        "status", "狀態", "select", false, true, None, None, None, true,
    );
    match entity {
        "purchase_order" => [
            vec![
                supplier,
                (
                    "order_date",
                    "採購日期",
                    "date",
                    true,
                    false,
                    None,
                    None,
                    None,
                    true,
                ),
                (
                    "expected_date",
                    "預計到貨日",
                    "date",
                    false,
                    false,
                    None,
                    None,
                    None,
                    true,
                ),
                currency,
                rate,
                tax,
                status,
            ],
            amounts,
            vec![notes],
        ]
        .concat(),
        "purchase_order_line" => vec![
            product,
            qty,
            price,
            amount,
            (
                "received_qty",
                "已進貨數量",
                "decimal",
                false,
                true,
                Some(4),
                None,
                None,
                true,
            ),
            notes,
        ],
        "goods_receipt" => [
            vec![
                supplier,
                (
                    "purchase_order_id",
                    "採購單",
                    "relation",
                    false,
                    false,
                    None,
                    Some("purchase_order"),
                    None,
                    true,
                ),
                (
                    "receipt_date",
                    "進貨日期",
                    "date",
                    true,
                    false,
                    None,
                    None,
                    None,
                    true,
                ),
                currency,
                rate,
                tax,
                status,
            ],
            amounts,
            vec![notes],
        ]
        .concat(),
        "goods_receipt_line" => vec![
            (
                "purchase_order_line_id",
                "採購明細",
                "relation",
                false,
                false,
                None,
                Some("purchase_order_line"),
                None,
                false,
            ),
            product,
            qty,
            price,
            amount,
            (
                "lot_no", "批號", "text", false, false, None, None, None, true,
            ),
            cost,
            notes,
        ],
        "purchase_return" => [
            vec![
                (
                    "goods_receipt_id",
                    "進貨單",
                    "relation",
                    true,
                    false,
                    None,
                    Some("goods_receipt"),
                    None,
                    true,
                ),
                (
                    "supplier_id",
                    "廠商",
                    "relation",
                    false,
                    true,
                    None,
                    Some("supplier"),
                    None,
                    true,
                ),
                (
                    "return_date",
                    "退回日期",
                    "date",
                    true,
                    false,
                    None,
                    None,
                    None,
                    true,
                ),
                (
                    "currency_code",
                    "幣別",
                    "text",
                    false,
                    true,
                    None,
                    None,
                    Some("currency"),
                    true,
                ),
                (
                    "exchange_rate",
                    "匯率",
                    "decimal",
                    false,
                    true,
                    Some(6),
                    None,
                    None,
                    true,
                ),
                (
                    "tax_type", "稅別", "select", false, true, None, None, None, true,
                ),
                status,
            ],
            amounts,
            vec![
                (
                    "reason",
                    "退回原因",
                    "text",
                    false,
                    false,
                    None,
                    None,
                    None,
                    true,
                ),
                notes,
            ],
        ]
        .concat(),
        "purchase_return_line" => vec![
            (
                "goods_receipt_line_id",
                "進貨明細",
                "relation",
                true,
                false,
                None,
                Some("goods_receipt_line"),
                None,
                false,
            ),
            (
                "product_id",
                "商品",
                "relation",
                false,
                true,
                None,
                Some("product"),
                None,
                true,
            ),
            (
                "lot_no", "批號", "text", false, true, None, None, None, true,
            ),
            qty,
            (
                "unit_price",
                "單價",
                "money",
                false,
                true,
                Some(4),
                None,
                None,
                true,
            ),
            amount,
            cost,
            notes,
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

/// 系統欄位的定義與 §8.8 的表一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn system_fields_match_spec(pool: PgPool) {
    for entity in [
        "purchase_order",
        "purchase_order_line",
        "goods_receipt",
        "goods_receipt_line",
        "purchase_return",
        "purchase_return_line",
    ] {
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
        let specs = specs(entity);
        let keys: Vec<&str> = rows.iter().map(|r| r.field_key.as_str()).collect();
        let spec_keys: Vec<&str> = specs.iter().map(|s| s.0).collect();
        assert_eq!(keys, spec_keys, "{entity} 的欄位與順序");

        for (i, (row, spec)) in rows.iter().zip(&specs).enumerate() {
            let at = format!("{entity}.{}", spec.0);
            assert_eq!(row.label, spec.1, "{at}");
            assert_eq!(row.data_type, spec.2, "{at}");
            assert!(row.is_system && row.is_active, "{at}");
            assert_eq!(row.storage, "column", "{at}");
            assert_eq!(row.column_name.as_deref(), Some(spec.0), "{at}");
            assert_eq!(row.is_required, spec.3, "{at} is_required");
            assert_eq!(row.is_readonly, spec.4, "{at} is_readonly");
            assert_eq!(row.decimal_places, spec.5, "{at}");
            assert_eq!(row.relation_key.as_deref(), spec.6, "{at}");
            assert_eq!(row.ui_widget.as_deref(), spec.7, "{at}");
            assert_eq!(row.is_visible, spec.8, "{at} is_visible");
            assert!(!row.is_unique && !row.is_searchable, "{at}");
            assert_eq!(row.default_value, None, "{at} default_value");
            assert_eq!(row.sort_order, (i as i32 + 1) * 10, "{at} sort_order");

            // 可輸入的數量、單價、匯率不可小於 0；批號最長 50 字元
            let expected_validation = match (spec.0, spec.4) {
                ("qty" | "unit_price" | "exchange_rate", false) => Some(json!({ "min": "0" })),
                ("lot_no", false) => Some(json!({ "max_length": 50 })),
                _ => None,
            };
            assert_eq!(row.validation, expected_validation, "{at} validation");
            if let Some(v) = &row.validation {
                serde_json::from_value::<Validation>(v.clone()).unwrap();
            }

            let options: Option<Vec<SelectOption>> = row
                .options
                .clone()
                .map(|v| serde_json::from_value(v).unwrap());
            let values: Option<Vec<&str>> = options
                .as_ref()
                .map(|o| o.iter().map(|o| o.value.as_str()).collect());
            let expected_options = match spec.0 {
                "tax_type" => Some(TAX.to_vec()),
                "status" if entity == "purchase_order" => Some(PO_STATUS.to_vec()),
                "status" => Some(STATUS.to_vec()),
                _ => None,
            };
            assert_eq!(values, expected_options, "{at} options");
        }
    }
}

/// 實體欄位的順序：id → 單號或父資料欄位 → 系統欄位 → 不是欄位的值 → custom_data、formula_errors、
/// 共通欄位。
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
    const POSTING: &[&str] = &["posted_at", "posted_by", "voided_at", "voided_by"];
    for (table, entity, head, after) in [
        (
            "purchase_orders",
            "purchase_order",
            &["id", "record_no"][..],
            [POSTING, &["closed_at", "closed_by"]].concat(),
        ),
        (
            "purchase_order_lines",
            "purchase_order_line",
            &["id", "purchase_order_id", "sort_order"][..],
            vec![],
        ),
        (
            "goods_receipts",
            "goods_receipt",
            &["id", "record_no"][..],
            POSTING.to_vec(),
        ),
        (
            "goods_receipt_lines",
            "goods_receipt_line",
            &["id", "goods_receipt_id", "sort_order"][..],
            vec!["lot_id"],
        ),
        (
            "purchase_returns",
            "purchase_return",
            &["id", "record_no"][..],
            POSTING.to_vec(),
        ),
        (
            "purchase_return_lines",
            "purchase_return_line",
            &["id", "purchase_return_id", "sort_order"][..],
            vec!["lot_id"],
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
        let fields: Vec<&str> = specs(entity).iter().map(|s| s.0).collect();
        let expected: Vec<&str> = [head, &fields, &after, TAIL].concat();
        assert_eq!(columns, expected, "{table}");
    }

    // 數值欄位的精度（§8.1）：匯率 (18,6)、台幣總計 (18,2)、其他 (18,4)
    let numeric: Vec<(String, String, i32, i32)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text, numeric_precision::int, numeric_scale::int
         FROM information_schema.columns
         WHERE table_schema = current_schema() AND data_type = 'numeric'
           AND table_name IN ('purchase_orders', 'purchase_order_lines', 'goods_receipts',
                              'goods_receipt_lines', 'purchase_returns', 'purchase_return_lines')
         ORDER BY table_name, column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(numeric.len(), 27, "{numeric:?}");
    for (table, column, precision, scale) in numeric {
        let expected = match column.as_str() {
            "exchange_rate" => 6,
            "total_twd" => 2,
            _ => 4,
        };
        assert_eq!((precision, scale), (18, expected), "{table}.{column}");
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
    let mut expected = vec![
        "idx_purchase_orders_order_date".to_owned(),
        "idx_goods_receipts_receipt_date".to_owned(),
        "idx_goods_receipts_purchase_order_id".to_owned(),
        "idx_purchase_returns_return_date".to_owned(),
        "idx_purchase_returns_goods_receipt_id".to_owned(),
        "idx_goods_receipt_lines_purchase_order_line_id".to_owned(),
        "idx_goods_receipt_lines_lot_id".to_owned(),
        "idx_purchase_return_lines_goods_receipt_line_id".to_owned(),
        "idx_purchase_return_lines_lot_id".to_owned(),
    ];
    for table in ["purchase_orders", "goods_receipts", "purchase_returns"] {
        for name in [
            "uq_{t}_record_no",
            "idx_{t}_status",
            "idx_{t}_supplier_id",
            "idx_{t}_custom_data",
        ] {
            expected.push(name.replace("{t}", table));
        }
    }
    for (table, parent) in [
        ("purchase_order_lines", "purchase_order_id"),
        ("goods_receipt_lines", "goods_receipt_id"),
        ("purchase_return_lines", "purchase_return_id"),
    ] {
        expected.push(format!("idx_{table}_{parent}"));
        expected.push(format!("idx_{table}_product_id"));
        expected.push(format!("idx_{table}_custom_data"));
    }
    for name in &expected {
        assert!(indexes.contains(name), "缺少索引 {name}");
    }

    for table in [
        "purchase_orders",
        "purchase_order_lines",
        "goods_receipts",
        "goods_receipt_lines",
        "purchase_returns",
        "purchase_return_lines",
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

/// 狀態與確認、作廢、結案資訊的 CHECK；確認、過帳後必須有匯率；金額預設 0。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn document_checks(pool: PgPool) {
    let (supplier, product, user) = seed(&pool).await;
    let po = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO purchase_orders (id, record_no, supplier_id, order_date, currency_code, tax_type)
         VALUES ($1, 'PO-1', $2, '2026-11-18', 'USD', 'zero_rated')",
    )
    .bind(po)
    .bind(supplier)
    .execute(&pool)
    .await
    .unwrap();
    let defaults: (String, Decimal, Decimal, Decimal, Option<Decimal>) = sqlx::query_as(
        "SELECT status, subtotal, tax_amount, total, total_twd FROM purchase_orders WHERE id = $1",
    )
    .bind(po)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        defaults,
        (
            "draft".to_owned(),
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            None
        )
    );

    let update = |sql: &str| {
        sql.replace("{po}", &po.to_string())
            .replace("{user}", &user.to_string())
    };
    for (sql, constraint) in [
        // 確認後必須有匯率
        (
            "UPDATE purchase_orders SET status = 'confirmed', posted_at = now(), posted_by = '{user}' WHERE id = '{po}'",
            "purchase_orders_confirmed_rate_check",
        ),
        // 非草稿必須有確認時間
        (
            "UPDATE purchase_orders SET status = 'confirmed', exchange_rate = 32 WHERE id = '{po}'",
            "purchase_orders_posted_status_check",
        ),
        // 手動結案只能是 closed
        (
            "UPDATE purchase_orders SET exchange_rate = 32, status = 'confirmed', posted_at = now(),
                 posted_by = '{user}', closed_at = now(), closed_by = '{user}' WHERE id = '{po}'",
            "purchase_orders_closed_status_check",
        ),
        (
            "UPDATE purchase_orders SET exchange_rate = 32, status = 'closed', posted_at = now(),
                 posted_by = '{user}', closed_at = now() WHERE id = '{po}'",
            "purchase_orders_closed_by_check",
        ),
        (
            "UPDATE purchase_orders SET status = 'posted', exchange_rate = 32, posted_at = now(),
                 posted_by = '{user}' WHERE id = '{po}'",
            "purchase_orders_status_check",
        ),
        ("UPDATE purchase_orders SET exchange_rate = 0 WHERE id = '{po}'", "purchase_orders_exchange_rate_check"),
    ] {
        let err = exec(&pool, &update(sql)).await.unwrap_err();
        assert_eq!(sqlstate(&err), ("23514".to_owned(), Some(constraint.to_owned())), "{sql}");
    }
    exec(
        &pool,
        &update(
            "UPDATE purchase_orders SET exchange_rate = 32, status = 'closed', posted_at = now(),
                 posted_by = '{user}', closed_at = now(), closed_by = '{user}' WHERE id = '{po}'",
        ),
    )
    .await
    .unwrap();

    // 明細：數量 > 0、單價 >= 0、已進貨數量 >= 0
    for (qty, price, received, constraint) in [
        ("0", "1", "0", "purchase_order_lines_qty_check"),
        ("1", "-1", "0", "purchase_order_lines_unit_price_check"),
        ("1", "1", "-1", "purchase_order_lines_received_qty_check"),
    ] {
        let err = sqlx::query(
            "INSERT INTO purchase_order_lines (id, purchase_order_id, product_id, qty, unit_price, received_qty)
             VALUES ($1, $2, $3, $4::numeric, $5::numeric, $6::numeric)",
        )
        .bind(Uuid::now_v7())
        .bind(po)
        .bind(product)
        .bind(qty)
        .bind(price)
        .bind(received)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(
            sqlstate(&err),
            ("23514".to_owned(), Some(constraint.to_owned()))
        );
    }

    // 進貨單：過帳後必須有匯率
    let (gr, _) = insert_receipt(&pool, supplier, product, "GR-1").await;
    let err = sqlx::query(
        "UPDATE goods_receipts SET exchange_rate = NULL, status = 'posted', posted_at = now(),
             posted_by = $2 WHERE id = $1",
    )
    .bind(gr)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23514".to_owned(),
            Some("goods_receipts_posted_rate_check".to_owned())
        )
    );

    // 單號唯一（含已刪除的資料）
    exec(&pool, "UPDATE goods_receipts SET deleted_at = now()")
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO goods_receipts (id, record_no, supplier_id, receipt_date, currency_code, tax_type)
         VALUES ($1, 'GR-1', $2, '2026-11-10', 'TWD', 'taxable')",
    )
    .bind(Uuid::now_v7())
    .bind(supplier)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23505".to_owned(),
            Some("uq_goods_receipts_record_no".to_owned())
        )
    );
}

/// 庫存的來源類型加上進貨明細、進貨退回明細與兩張單據。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn inventory_accepts_purchasing_sources(pool: PgPool) {
    let (_, product, _) = seed(&pool).await;
    let lot = NewLot {
        id: Uuid::now_v7(),
        product_id: product,
        lot_no: "L-01",
        received_date: date!(2026 - 11 - 10),
        source_type: "goods_receipt_line",
        source_doc_id: Uuid::now_v7(),
        source_line_id: Uuid::now_v7(),
        qty: dec("10"),
        unit_cost_twd: dec("396.8918"),
        created_by: None,
    };
    inventory::insert_lot(&pool, &lot).await.unwrap();
    for (txn_type, ref_doc_type, qty) in [
        ("receipt", "goods_receipt", "10"),
        ("purchase_return", "purchase_return", "-2"),
    ] {
        inventory::insert_transaction(
            &pool,
            &NewTransaction {
                id: Uuid::now_v7(),
                txn_date: date!(2026 - 11 - 18),
                txn_type,
                product_id: product,
                lot_id: lot.id,
                qty_change: dec(qty),
                unit_cost_twd: lot.unit_cost_twd,
                ref_doc_type,
                ref_doc_id: Uuid::now_v7(),
                ref_line_id: Uuid::now_v7(),
                created_by: None,
            },
        )
        .await
        .unwrap();
    }
    let ok = sqlx::query(
        "INSERT INTO lot_allocations (id, source_type, source_doc_id, source_line_id, lot_id, qty, unit_cost_twd)
         VALUES ($1, 'purchase_return_line', $2, $3, $4, 2, 396.8918)",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(lot.id)
    .execute(&pool)
    .await;
    ok.unwrap();
    // 進貨明細不是扣除庫存的來源
    let err = sqlx::query(
        "INSERT INTO lot_allocations (id, source_type, source_doc_id, source_line_id, lot_id, qty, unit_cost_twd)
         VALUES ($1, 'goods_receipt_line', $2, $3, $4, 1, 1)",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(lot.id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23514".to_owned(),
            Some("lot_allocations_source_type_check".to_owned())
        )
    );
}

// ---------------------------------------------------------------------------
// hammer_db::purchasing
// ---------------------------------------------------------------------------

/// 進貨退回帶入用的進貨單與進貨明細：已刪除的不回傳、不屬於該進貨單的明細不回傳。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn receipt_access_for_return(pool: PgPool) {
    let (supplier, product, _) = seed(&pool).await;
    let (gr, line) = insert_receipt(&pool, supplier, product, "GR-1").await;
    let (other, other_line) = insert_receipt(&pool, supplier, product, "GR-2").await;

    let receipt = purchasing::find_receipt_for_return(&pool, gr)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, "draft");
    assert_eq!(receipt.supplier_id, supplier);
    assert_eq!(receipt.currency_code, "USD");
    assert_eq!(receipt.exchange_rate, Some(dec("32.150000")));
    assert_eq!(receipt.exchange_rate.unwrap().to_string(), "32.150000");
    assert_eq!(receipt.tax_type, "zero_rated");

    let rows = purchasing::list_receipt_lines_for_return(&pool, gr, &[line, other_line])
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, line);
    assert_eq!(rows[0].product_id, product);
    assert_eq!(rows[0].lot_no.as_deref(), Some("L-01"));
    assert_eq!(rows[0].unit_price.to_string(), "12.3450");
    assert_eq!(rows[0].lot_id, None);

    exec(
        &pool,
        &format!("UPDATE goods_receipts SET deleted_at = now() WHERE id = '{other}'"),
    )
    .await
    .unwrap();
    assert_eq!(
        purchasing::find_receipt_for_return(&pool, other)
            .await
            .unwrap(),
        None
    );
    exec(
        &pool,
        &format!("UPDATE goods_receipt_lines SET deleted_at = now() WHERE id = '{line}'"),
    )
    .await
    .unwrap();
    assert!(
        purchasing::list_receipt_lines_for_return(&pool, gr, &[line])
            .await
            .unwrap()
            .is_empty()
    );
}
