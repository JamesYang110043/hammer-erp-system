//! 0013_sales 的整合測試：訂單、出貨單、銷貨退回的資料表與資料類別、欄位定義的登記，
//! 庫存來源類型的擴充，以及 `hammer_db::sales` 的存取函式（docs/architecture.md §8.9）。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use hammer_db::inventory::{self, NewLot, NewTransaction};
use hammer_db::sales;
use hammer_db::sqlx::{self, PgPool, migrate::Migrator};
use hammer_shared::schema::{SelectOption, Validation};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use time::macros::date;
use uuid::Uuid;

/// 0013 之前的最後一個 migration 版本。
const BEFORE_SALES: i64 = 12;

const ENTITIES: [&str; 6] = [
    "sales_order",
    "sales_order_line",
    "shipment",
    "shipment_line",
    "sales_return",
    "sales_return_line",
];

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

/// 客戶、商品、使用者，回傳 `(customer_id, product_id, user_id)`。
async fn seed(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let customer = Uuid::now_v7();
    sqlx::query("INSERT INTO customers (id, code, name) VALUES ($1, 'C1', '客戶')")
        .bind(customer)
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
    (customer, product, user)
}

/// 新增一張出貨單（草稿，單價含稅）與一列明細，回傳 `(shipment_id, line_id)`。
async fn insert_shipment(pool: &PgPool, customer: Uuid, product: Uuid, no: &str) -> (Uuid, Uuid) {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO shipments (id, record_no, customer_id, ship_date, tax_type, tax_included)
         VALUES ($1, $2, $3, '2026-11-10', 'taxable', true)",
    )
    .bind(id)
    .bind(no)
    .bind(customer)
    .execute(pool)
    .await
    .unwrap();
    let line = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO shipment_lines (id, shipment_id, product_id, qty, unit_price, discount_rate, lot_no)
         VALUES ($1, $2, $3, 10, 105, 0.1, 'L-01')",
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

/// 在 M4b 的資料庫上套用 0013：既有資料保留，銷貨單據登記為系統資料類別。
#[sqlx::test(migrations = false)]
async fn applies_on_existing_m4b_database(pool: PgPool) {
    let before = Migrator::with_migrations(
        hammer_db::MIGRATOR
            .iter()
            .filter(|m| m.version <= BEFORE_SALES)
            .cloned()
            .collect(),
    );
    before.run(&pool).await.unwrap();
    let (customer, product, _) = seed(&pool).await;

    hammer_db::run_migrations(&pool).await.unwrap();
    hammer_db::run_migrations(&pool).await.unwrap();

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entity_definitions WHERE key = ANY($1)")
            .bind(&ENTITIES[..])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 6);
    insert_shipment(&pool, customer, product, "SH-1").await;
}

// ---------------------------------------------------------------------------
// 資料類別與欄位定義
// ---------------------------------------------------------------------------

/// 六個資料類別與 §8.9「資料類別」的表一致。
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
         WHERE e.key = ANY($1)
         ORDER BY e.id",
    )
    .bind(&ENTITIES[..])
    .fetch_all(&pool)
    .await
    .unwrap();
    let doc_title = "{record_no} {customer_id.name}";
    let line_title = "{product_id.code} {product_id.name}";
    let expected = [
        (
            "sales_order",
            "訂單",
            "document",
            "sales_orders",
            None,
            Some("SO{yyyyMM}-{0000}"),
            doc_title,
            200,
        ),
        (
            "sales_order_line",
            "訂單明細",
            "detail",
            "sales_order_lines",
            Some("sales_order"),
            None,
            line_title,
            0,
        ),
        (
            "shipment",
            "出貨單",
            "document",
            "shipments",
            None,
            Some("SH{yyyyMM}-{0000}"),
            doc_title,
            210,
        ),
        (
            "shipment_line",
            "出貨明細",
            "detail",
            "shipment_lines",
            Some("shipment"),
            None,
            "{product_id.code} {product_id.name} {lot_no}",
            0,
        ),
        (
            "sales_return",
            "銷貨退回",
            "document",
            "sales_returns",
            None,
            Some("SR{yyyyMM}-{0000}"),
            doc_title,
            220,
        ),
        (
            "sales_return_line",
            "銷貨退回明細",
            "detail",
            "sales_return_lines",
            Some("sales_return"),
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

/// 欄位定義的預期值。
#[derive(Debug, Clone)]
struct Spec {
    key: &'static str,
    label: &'static str,
    data_type: &'static str,
    required: bool,
    readonly: bool,
    decimals: Option<i16>,
    relation: Option<&'static str>,
    visible: bool,
    default_value: Option<Value>,
}

/// 可輸入的欄位。
fn input(key: &'static str, label: &'static str, data_type: &'static str, required: bool) -> Spec {
    Spec {
        key,
        label,
        data_type,
        required,
        readonly: false,
        decimals: None,
        relation: None,
        visible: true,
        default_value: None,
    }
}

/// 系統維護的欄位（唯讀、不可必填）。
fn maintained(key: &'static str, label: &'static str, data_type: &'static str) -> Spec {
    Spec {
        readonly: true,
        ..input(key, label, data_type, false)
    }
}

impl Spec {
    fn decimals(mut self, n: i16) -> Self {
        self.decimals = Some(n);
        self
    }
    fn relation(mut self, entity: &'static str) -> Self {
        self.relation = Some(entity);
        self
    }
    fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }
    fn default(mut self, value: Value) -> Self {
        self.default_value = Some(value);
        self
    }
}

const TAX: &[&str] = &["taxable", "zero_rated", "exempt"];
const STATUS: &[&str] = &["draft", "posted", "void"];
const SO_STATUS: &[&str] = &["draft", "confirmed", "partially_shipped", "closed", "void"];

fn specs(entity: &str) -> Vec<Spec> {
    let amounts = || {
        vec![
            maintained("subtotal", "未稅金額", "money").decimals(4),
            maintained("tax_amount", "稅額", "money").decimals(4),
            maintained("total", "總計", "money").decimals(4),
        ]
    };
    let notes = || input("notes", "備註", "long_text", false);
    let tax_type = || input("tax_type", "稅別", "select", true);
    let tax_included = || input("tax_included", "單價含稅", "boolean", true).default(json!(false));
    let status = || maintained("status", "狀態", "select");
    let customer = || input("customer_id", "客戶", "relation", true).relation("customer");
    let product = || input("product_id", "商品", "relation", true).relation("product");
    let qty = || input("qty", "數量", "decimal", true).decimals(4);
    let unit_price = || input("unit_price", "單價", "money", true).decimals(4);
    let discount = || {
        input("discount_rate", "折扣率", "decimal", true)
            .decimals(4)
            .default(json!("0"))
    };
    let amount = || maintained("amount", "金額", "money").decimals(4);
    let cost = || maintained("cost_amount_twd", "成本", "money").decimals(2);
    match entity {
        "sales_order" => [
            vec![
                customer(),
                input("order_date", "訂單日期", "date", true),
                input("delivery_date", "預計交貨日", "date", false),
                tax_type(),
                tax_included(),
                status(),
            ],
            amounts(),
            vec![notes()],
        ]
        .concat(),
        "sales_order_line" => vec![
            product(),
            qty(),
            unit_price(),
            discount(),
            amount(),
            maintained("shipped_qty", "已出貨數量", "decimal").decimals(4),
            notes(),
        ],
        "shipment" => [
            vec![
                customer(),
                input("sales_order_id", "訂單", "relation", false).relation("sales_order"),
                input("ship_date", "出貨日期", "date", true),
                tax_type(),
                tax_included(),
                status(),
            ],
            amounts(),
            vec![
                maintained("cost_total_twd", "銷貨成本", "money").decimals(2),
                input("ship_address", "送貨地址", "text", false),
                notes(),
            ],
        ]
        .concat(),
        "shipment_line" => vec![
            input("sales_order_line_id", "訂單明細", "relation", false)
                .relation("sales_order_line")
                .hidden(),
            product(),
            qty(),
            unit_price(),
            discount(),
            amount(),
            input("lot_no", "批號", "text", false),
            cost(),
            notes(),
        ],
        "sales_return" => [
            vec![
                input("shipment_id", "出貨單", "relation", true).relation("shipment"),
                maintained("customer_id", "客戶", "relation").relation("customer"),
                input("return_date", "退回日期", "date", true),
                maintained("tax_type", "稅別", "select"),
                maintained("tax_included", "單價含稅", "boolean"),
                status(),
            ],
            amounts(),
            vec![
                maintained("cost_total_twd", "退回成本", "money").decimals(2),
                input("reason", "退回原因", "text", false),
                notes(),
            ],
        ]
        .concat(),
        "sales_return_line" => vec![
            input("shipment_line_id", "出貨明細", "relation", true)
                .relation("shipment_line")
                .hidden(),
            maintained("product_id", "商品", "relation").relation("product"),
            qty(),
            maintained("unit_price", "單價", "money").decimals(4),
            maintained("discount_rate", "折扣率", "decimal").decimals(4),
            amount(),
            cost(),
            notes(),
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

/// 系統欄位的定義與 §8.9 的表一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn system_fields_match_spec(pool: PgPool) {
    for entity in ENTITIES {
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
        let spec_keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
        assert_eq!(keys, spec_keys, "{entity} 的欄位與順序");

        for (i, (row, spec)) in rows.iter().zip(&specs).enumerate() {
            let at = format!("{entity}.{}", spec.key);
            assert_eq!(row.label, spec.label, "{at}");
            assert_eq!(row.data_type, spec.data_type, "{at}");
            assert!(row.is_system && row.is_active, "{at}");
            assert_eq!(row.storage, "column", "{at}");
            assert_eq!(row.column_name.as_deref(), Some(spec.key), "{at}");
            assert_eq!(row.is_required, spec.required, "{at} is_required");
            assert_eq!(row.is_readonly, spec.readonly, "{at} is_readonly");
            assert_eq!(row.decimal_places, spec.decimals, "{at}");
            assert_eq!(row.relation_key.as_deref(), spec.relation, "{at}");
            assert_eq!(row.ui_widget, None, "{at}");
            assert_eq!(row.is_visible, spec.visible, "{at} is_visible");
            assert!(!row.is_unique && !row.is_searchable, "{at}");
            assert_eq!(row.default_value, spec.default_value, "{at} default_value");
            assert_eq!(row.sort_order, (i as i32 + 1) * 10, "{at} sort_order");

            // 可輸入的數量、單價不可小於 0；折扣率介於 0 與 1；批號最長 50 字元
            let expected_validation = match (spec.key, spec.readonly) {
                ("qty" | "unit_price", false) => Some(json!({ "min": "0" })),
                ("discount_rate", false) => Some(json!({ "min": "0", "max": "1" })),
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
            let expected_options = match spec.key {
                "tax_type" => Some(TAX.to_vec()),
                "status" if entity == "sales_order" => Some(SO_STATUS.to_vec()),
                "status" => Some(STATUS.to_vec()),
                _ => None,
            };
            assert_eq!(values, expected_options, "{at} options");
        }
    }
}

/// 實體欄位的順序：id → 單號或父資料欄位 → 系統欄位 → 不是欄位的值 → custom_data、formula_errors、
/// 共通欄位；數值欄位的精度。
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
            "sales_orders",
            "sales_order",
            &["id", "record_no"][..],
            [POSTING, &["closed_at", "closed_by"]].concat(),
        ),
        (
            "sales_order_lines",
            "sales_order_line",
            &["id", "sales_order_id", "sort_order"][..],
            vec![],
        ),
        (
            "shipments",
            "shipment",
            &["id", "record_no"][..],
            POSTING.to_vec(),
        ),
        (
            "shipment_lines",
            "shipment_line",
            &["id", "shipment_id", "sort_order"][..],
            vec!["lot_id"],
        ),
        (
            "sales_returns",
            "sales_return",
            &["id", "record_no"][..],
            POSTING.to_vec(),
        ),
        (
            "sales_return_lines",
            "sales_return_line",
            &["id", "sales_return_id", "sort_order"][..],
            vec![],
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
        let fields: Vec<&str> = specs(entity).iter().map(|s| s.key).collect();
        let expected: Vec<&str> = [head, &fields, &after, TAIL].concat();
        assert_eq!(columns, expected, "{table}");
    }

    // 數值欄位的精度（§8.1）：折扣率 (6,4)、成本 (18,2)、其他 (18,4)
    let numeric: Vec<(String, String, i32, i32)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text, numeric_precision::int, numeric_scale::int
         FROM information_schema.columns
         WHERE table_schema = current_schema() AND data_type = 'numeric'
           AND table_name IN ('sales_orders', 'sales_order_lines', 'shipments', 'shipment_lines',
                              'sales_returns', 'sales_return_lines')
         ORDER BY table_name, column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(numeric.len(), 26, "{numeric:?}");
    for (table, column, precision, scale) in numeric {
        let expected = match column.as_str() {
            "discount_rate" => (6, 4),
            "cost_total_twd" | "cost_amount_twd" => (18, 2),
            _ => (18, 4),
        };
        assert_eq!((precision, scale), expected, "{table}.{column}");
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
        "idx_sales_orders_order_date".to_owned(),
        "idx_shipments_ship_date".to_owned(),
        "idx_shipments_sales_order_id".to_owned(),
        "idx_sales_returns_return_date".to_owned(),
        "idx_sales_returns_shipment_id".to_owned(),
        "idx_shipment_lines_sales_order_line_id".to_owned(),
        "idx_shipment_lines_lot_id".to_owned(),
        "idx_sales_return_lines_shipment_line_id".to_owned(),
    ];
    for table in ["sales_orders", "shipments", "sales_returns"] {
        for name in [
            "uq_{t}_record_no",
            "idx_{t}_status",
            "idx_{t}_customer_id",
            "idx_{t}_custom_data",
        ] {
            expected.push(name.replace("{t}", table));
        }
    }
    for (table, parent) in [
        ("sales_order_lines", "sales_order_id"),
        ("shipment_lines", "shipment_id"),
        ("sales_return_lines", "sales_return_id"),
    ] {
        expected.push(format!("idx_{table}_{parent}"));
        expected.push(format!("idx_{table}_product_id"));
        expected.push(format!("idx_{table}_custom_data"));
    }
    for name in &expected {
        assert!(indexes.contains(name), "缺少索引 {name}");
    }

    for table in [
        "sales_orders",
        "sales_order_lines",
        "shipments",
        "shipment_lines",
        "sales_returns",
        "sales_return_lines",
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

/// 狀態與確認、作廢、結案資訊的 CHECK；金額預設 0；明細的數量、單價、折扣率；單號唯一。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn document_checks(pool: PgPool) {
    let (customer, product, user) = seed(&pool).await;
    let so = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales_orders (id, record_no, customer_id, order_date, tax_type)
         VALUES ($1, 'SO-1', $2, '2026-11-18', 'taxable')",
    )
    .bind(so)
    .bind(customer)
    .execute(&pool)
    .await
    .unwrap();
    let defaults: (String, bool, Decimal, Decimal, Decimal) = sqlx::query_as(
        "SELECT status, tax_included, subtotal, tax_amount, total FROM sales_orders WHERE id = $1",
    )
    .bind(so)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        defaults,
        (
            "draft".to_owned(),
            false,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO
        )
    );

    let update = |sql: &str| {
        sql.replace("{so}", &so.to_string())
            .replace("{user}", &user.to_string())
    };
    for (sql, constraint) in [
        // 非草稿必須有確認時間
        (
            "UPDATE sales_orders SET status = 'confirmed' WHERE id = '{so}'",
            "sales_orders_posted_status_check",
        ),
        // 手動結案只能是 closed
        (
            "UPDATE sales_orders SET status = 'confirmed', posted_at = now(), posted_by = '{user}',
                 closed_at = now(), closed_by = '{user}' WHERE id = '{so}'",
            "sales_orders_closed_status_check",
        ),
        (
            "UPDATE sales_orders SET status = 'closed', posted_at = now(), posted_by = '{user}',
                 closed_at = now() WHERE id = '{so}'",
            "sales_orders_closed_by_check",
        ),
        (
            "UPDATE sales_orders SET status = 'posted', posted_at = now(), posted_by = '{user}'
             WHERE id = '{so}'",
            "sales_orders_status_check",
        ),
        (
            "UPDATE sales_orders SET status = 'void', posted_at = now(), posted_by = '{user}'
             WHERE id = '{so}'",
            "sales_orders_voided_status_check",
        ),
    ] {
        let err = exec(&pool, &update(sql)).await.unwrap_err();
        assert_eq!(
            sqlstate(&err),
            ("23514".to_owned(), Some(constraint.to_owned())),
            "{sql}"
        );
    }
    exec(
        &pool,
        &update(
            "UPDATE sales_orders SET status = 'partially_shipped', posted_at = now(),
                 posted_by = '{user}' WHERE id = '{so}'",
        ),
    )
    .await
    .unwrap();

    // 明細：數量 > 0、單價 >= 0、折扣率介於 0 與 1、已出貨數量 >= 0
    for (qty, price, discount, shipped, constraint) in [
        ("0", "1", "0", "0", "sales_order_lines_qty_check"),
        ("1", "-1", "0", "0", "sales_order_lines_unit_price_check"),
        (
            "1",
            "1",
            "-0.1",
            "0",
            "sales_order_lines_discount_rate_check",
        ),
        (
            "1",
            "1",
            "1.1",
            "0",
            "sales_order_lines_discount_rate_check",
        ),
        ("1", "1", "0", "-1", "sales_order_lines_shipped_qty_check"),
    ] {
        let err = sqlx::query(
            "INSERT INTO sales_order_lines
                 (id, sales_order_id, product_id, qty, unit_price, discount_rate, shipped_qty)
             VALUES ($1, $2, $3, $4::numeric, $5::numeric, $6::numeric, $7::numeric)",
        )
        .bind(Uuid::now_v7())
        .bind(so)
        .bind(product)
        .bind(qty)
        .bind(price)
        .bind(discount)
        .bind(shipped)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(
            sqlstate(&err),
            ("23514".to_owned(), Some(constraint.to_owned()))
        );
    }
    // 折扣率可以是 1（全額折扣）
    exec(
        &pool,
        &format!(
            "INSERT INTO sales_order_lines (id, sales_order_id, product_id, qty, unit_price, discount_rate)
             VALUES ('{}', '{so}', '{product}', 1, 1, 1)",
            Uuid::now_v7()
        ),
    )
    .await
    .unwrap();

    // 出貨單：成本草稿為 NULL；過帳必須有過帳人員
    let (sh, line) = insert_shipment(&pool, customer, product, "SH-1").await;
    let costs: (Option<Decimal>, Option<Decimal>) = sqlx::query_as(
        "SELECT s.cost_total_twd, l.cost_amount_twd
         FROM shipments s JOIN shipment_lines l ON l.shipment_id = s.id WHERE s.id = $1",
    )
    .bind(sh)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(costs, (None, None));
    let err =
        sqlx::query("UPDATE shipments SET status = 'posted', posted_at = now() WHERE id = $1")
            .bind(sh)
            .execute(&pool)
            .await
            .unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23514".to_owned(),
            Some("shipments_posted_by_check".to_owned())
        )
    );

    // 銷貨退回的 tax_included 沒有預設值（存檔時從出貨單帶入）
    let err = sqlx::query(
        "INSERT INTO sales_returns (id, record_no, shipment_id, customer_id, return_date, tax_type)
         VALUES ($1, 'SR-1', $2, $3, '2026-11-18', 'taxable')",
    )
    .bind(Uuid::now_v7())
    .bind(sh)
    .bind(customer)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(sqlstate(&err).0, "23502");
    let sr = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales_returns (id, record_no, shipment_id, customer_id, return_date, tax_type,
                                    tax_included)
         VALUES ($1, 'SR-1', $2, $3, '2026-11-18', 'taxable', true)",
    )
    .bind(sr)
    .bind(sh)
    .bind(customer)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sales_return_lines (id, sales_return_id, shipment_line_id, product_id, qty,
                                         unit_price, discount_rate)
         VALUES ($1, $2, $3, $4, 2, 105, 0.1)",
    )
    .bind(Uuid::now_v7())
    .bind(sr)
    .bind(line)
    .bind(product)
    .execute(&pool)
    .await
    .unwrap();

    // 單號唯一（含已刪除的資料）
    exec(&pool, "UPDATE shipments SET deleted_at = now()")
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO shipments (id, record_no, customer_id, ship_date, tax_type)
         VALUES ($1, 'SH-1', $2, '2026-11-10', 'taxable')",
    )
    .bind(Uuid::now_v7())
    .bind(customer)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23505".to_owned(),
            Some("uq_shipments_record_no".to_owned())
        )
    );
}

/// 庫存的來源類型加上出貨明細與兩張單據；批號仍只能由進貨或盤點調整產生。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn inventory_accepts_sales_sources(pool: PgPool) {
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
        unit_cost_twd: dec("80"),
        created_by: None,
    };
    inventory::insert_lot(&pool, &lot).await.unwrap();
    for (txn_type, ref_doc_type, qty) in [
        ("receipt", "goods_receipt", "10"),
        ("shipment", "shipment", "-4"),
        ("sales_return", "sales_return", "1"),
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
    let insert_allocation = |source_type: &'static str| {
        sqlx::query(
            "INSERT INTO lot_allocations (id, source_type, source_doc_id, source_line_id, lot_id, qty, unit_cost_twd)
             VALUES ($1, $2, $3, $4, $5, 4, 80)",
        )
        .bind(Uuid::now_v7())
        .bind(source_type)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(lot.id)
        .execute(&pool)
    };
    insert_allocation("shipment_line").await.unwrap();
    // 銷貨退回是加回，不是扣除庫存的來源
    let err = insert_allocation("sales_return_line").await.unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23514".to_owned(),
            Some("lot_allocations_source_type_check".to_owned())
        )
    );
    // 出貨不產生批號
    let err = inventory::insert_lot(
        &pool,
        &NewLot {
            id: Uuid::now_v7(),
            lot_no: "L-02",
            source_type: "shipment_line",
            ..lot
        },
    )
    .await
    .unwrap_err();
    assert!(format!("{err:?}").contains("inventory_lots_source_type_check"));
}

// ---------------------------------------------------------------------------
// hammer_db::sales
// ---------------------------------------------------------------------------

/// 銷貨退回帶入用的出貨單與出貨明細：已刪除的不回傳、不屬於該出貨單的明細不回傳。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn shipment_access_for_return(pool: PgPool) {
    let (customer, product, _) = seed(&pool).await;
    let (sh, line) = insert_shipment(&pool, customer, product, "SH-1").await;
    let (other, other_line) = insert_shipment(&pool, customer, product, "SH-2").await;

    let shipment = sales::find_shipment_for_return(&pool, sh)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shipment.status, "draft");
    assert_eq!(shipment.customer_id, customer);
    assert_eq!(shipment.tax_type, "taxable");
    assert!(shipment.tax_included);

    let rows = sales::list_shipment_lines_for_return(&pool, sh, &[line, other_line])
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, line);
    assert_eq!(rows[0].product_id, product);
    assert_eq!(rows[0].unit_price.to_string(), "105.0000");
    assert_eq!(rows[0].discount_rate.to_string(), "0.1000");

    exec(
        &pool,
        &format!("UPDATE shipments SET deleted_at = now() WHERE id = '{other}'"),
    )
    .await
    .unwrap();
    assert_eq!(
        sales::find_shipment_for_return(&pool, other).await.unwrap(),
        None
    );
    exec(
        &pool,
        &format!("UPDATE shipment_lines SET deleted_at = now() WHERE id = '{line}'"),
    )
    .await
    .unwrap();
    assert!(
        sales::list_shipment_lines_for_return(&pool, sh, &[line])
            .await
            .unwrap()
            .is_empty()
    );
}
