//! 0014_invoicing 的整合測試：銷項發票、銷貨折讓、進項憑證、進貨折讓的資料表與資料類別、欄位定義的登記，
//! 發票號碼的唯一索引，作廢日期與原因的 CHECK，以及 `hammer_db::invoicing` 的存取函式
//! （docs/architecture.md §8.11）。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use hammer_db::invoicing;
use hammer_db::sqlx::{self, PgPool, migrate::Migrator};
use hammer_shared::schema::SelectOption;
use serde_json::{Value, json};
use uuid::Uuid;

/// 0014 之前的最後一個 migration 版本。
const BEFORE_INVOICING: i64 = 13;

const ENTITIES: [&str; 8] = [
    "sales_invoice",
    "sales_invoice_line",
    "sales_invoice_shipment",
    "sales_allowance",
    "sales_allowance_line",
    "purchase_invoice",
    "purchase_invoice_receipt",
    "purchase_allowance",
];

fn sqlstate(err: &sqlx::Error) -> (String, Option<String>) {
    let db = err.as_database_error().expect("應為資料庫錯誤");
    (
        db.code().unwrap().into_owned(),
        db.constraint().map(str::to_owned),
    )
}

/// 客戶、廠商、使用者，回傳 `(customer_id, supplier_id, user_id)`。
async fn seed(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let customer = Uuid::now_v7();
    sqlx::query("INSERT INTO customers (id, code, name) VALUES ($1, 'C1', '客戶')")
        .bind(customer)
        .execute(pool)
        .await
        .unwrap();
    let supplier = Uuid::now_v7();
    sqlx::query("INSERT INTO suppliers (id, code, name) VALUES ($1, 'S1', '廠商')")
        .bind(supplier)
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
    (customer, supplier, user)
}

/// 新增一張銷項發票草稿，回傳 id。
async fn insert_sales_invoice(pool: &PgPool, customer: Uuid, record_no: &str, no: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales_invoices (id, record_no, invoice_no, invoice_date, tax_period, customer_id, tax_type)
         VALUES ($1, $2, $3, '2026-11-19', '202611', $4, 'taxable')",
    )
    .bind(id)
    .bind(record_no)
    .bind(no)
    .bind(customer)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// 新增一張進項憑證草稿，回傳 id。
async fn insert_purchase_invoice(pool: &PgPool, supplier: Uuid, record_no: &str, no: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO purchase_invoices
             (id, record_no, invoice_no, invoice_date, tax_period, supplier_id, tax_type, subtotal, tax_amount)
         VALUES ($1, $2, $3, '2026-11-19', '202611', $4, 'zero_rated', 100, 5)",
    )
    .bind(id)
    .bind(record_no)
    .bind(no)
    .bind(supplier)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn set_status(pool: &PgPool, table: &str, id: Uuid, status: &str, user: Uuid) {
    let sql = format!(
        "UPDATE {table} SET status = $2,
             posted_at = CASE WHEN $2 = 'draft' THEN NULL ELSE now() END,
             posted_by = CASE WHEN $2 = 'draft' THEN NULL ELSE $3 END,
             voided_at = CASE WHEN $2 = 'void' THEN now() END,
             voided_by = CASE WHEN $2 = 'void' THEN $3 END,
             void_date = CASE WHEN $2 = 'void' THEN DATE '2026-11-20' END,
             void_reason = CASE WHEN $2 = 'void' THEN '開錯' END
         WHERE id = $1"
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(status)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// migration
// ---------------------------------------------------------------------------

/// 在 M4c 的資料庫上套用 0014：既有資料保留，發票模組的單據登記為系統資料類別。
#[sqlx::test(migrations = false)]
async fn applies_on_existing_m4c_database(pool: PgPool) {
    let before = Migrator::with_migrations(
        hammer_db::MIGRATOR
            .iter()
            .filter(|m| m.version <= BEFORE_INVOICING)
            .cloned()
            .collect(),
    );
    before.run(&pool).await.unwrap();
    let (customer, _, _) = seed(&pool).await;

    hammer_db::run_migrations(&pool).await.unwrap();
    hammer_db::run_migrations(&pool).await.unwrap();

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entity_definitions WHERE key = ANY($1)")
            .bind(&ENTITIES[..])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 8);
    insert_sales_invoice(&pool, customer, "SI-1", "AB12345678").await;
}

// ---------------------------------------------------------------------------
// 資料類別與欄位定義
// ---------------------------------------------------------------------------

/// 八個資料類別與 §8.11「資料類別」的表一致。
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
    #[allow(clippy::type_complexity)]
    let expected: [(
        &str,
        &str,
        &str,
        &str,
        Option<&str>,
        Option<&str>,
        &str,
        i32,
    ); 8] = [
        (
            "sales_invoice",
            "銷項發票",
            "document",
            "sales_invoices",
            None,
            Some("SI{yyyyMM}-{0000}"),
            "{invoice_no} {buyer_name}",
            300,
        ),
        (
            "sales_invoice_line",
            "發票明細",
            "detail",
            "sales_invoice_lines",
            Some("sales_invoice"),
            None,
            "{description}",
            0,
        ),
        (
            "sales_invoice_shipment",
            "發票出貨單",
            "detail",
            "sales_invoice_shipments",
            Some("sales_invoice"),
            None,
            "{shipment_id.record_no}",
            0,
        ),
        (
            "sales_allowance",
            "銷貨折讓",
            "document",
            "sales_allowances",
            None,
            Some("SA{yyyyMM}-{0000}"),
            "{record_no} {customer_id.name}",
            310,
        ),
        (
            "sales_allowance_line",
            "折讓明細",
            "detail",
            "sales_allowance_lines",
            Some("sales_allowance"),
            None,
            "{description}",
            0,
        ),
        (
            "purchase_invoice",
            "進項憑證",
            "document",
            "purchase_invoices",
            None,
            Some("PI{yyyyMM}-{0000}"),
            "{invoice_no} {supplier_id.name}",
            320,
        ),
        (
            "purchase_invoice_receipt",
            "憑證進貨單",
            "detail",
            "purchase_invoice_receipts",
            Some("purchase_invoice"),
            None,
            "{goods_receipt_id.record_no}",
            0,
        ),
        (
            "purchase_allowance",
            "進貨折讓",
            "document",
            "purchase_allowances",
            None,
            Some("PA{yyyyMM}-{0000}"),
            "{record_no} {supplier_id.name}",
            330,
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

/// 欄位：(field_key, 顯示名稱, data_type, 必填, 系統維護, 小數位數, 關聯的資料類別)。
type Spec = (
    &'static str,
    &'static str,
    &'static str,
    bool,
    bool,
    Option<i16>,
    Option<&'static str>,
);

/// §8.11 各資料表的系統欄位（依順序）。
fn specs(entity: &str) -> Vec<Spec> {
    const M: Option<i16> = Some(4);
    let void = [
        ("void_date", "作廢日期", "date", false, true, None, None),
        ("void_reason", "作廢原因", "text", false, true, None, None),
    ];
    let notes = ("notes", "備註", "long_text", false, false, None, None);
    let text_lines = |_: ()| -> Vec<Spec> {
        vec![
            (
                "product_id",
                "商品",
                "relation",
                false,
                false,
                None,
                Some("product"),
            ),
            ("description", "品名", "text", true, false, None, None),
            ("qty", "數量", "decimal", true, false, M, None),
            ("unit_price", "單價", "money", true, false, M, None),
            ("amount", "金額", "money", false, true, M, None),
            notes,
        ]
    };
    match entity {
        "sales_invoice" => {
            let mut v = vec![
                (
                    "invoice_type",
                    "發票種類",
                    "select",
                    true,
                    false,
                    None,
                    None,
                ),
                ("invoice_no", "發票號碼", "text", true, false, None, None),
                ("invoice_date", "發票日期", "date", true, false, None, None),
                ("tax_period", "期別", "text", false, true, None, None),
                (
                    "customer_id",
                    "客戶",
                    "relation",
                    true,
                    false,
                    None,
                    Some("customer"),
                ),
                (
                    "buyer_tax_id",
                    "買受人統一編號",
                    "text",
                    false,
                    false,
                    None,
                    None,
                ),
                ("buyer_name", "買受人", "text", false, false, None, None),
                (
                    "buyer_address",
                    "買受人地址",
                    "text",
                    false,
                    false,
                    None,
                    None,
                ),
                ("tax_type", "稅別", "select", true, false, None, None),
                (
                    "tax_amount_manual",
                    "稅額（手動）",
                    "money",
                    false,
                    false,
                    M,
                    None,
                ),
                ("status", "狀態", "select", false, true, None, None),
                ("subtotal", "未稅金額", "money", false, true, M, None),
                ("tax_amount", "稅額", "money", false, true, M, None),
                ("total", "總計", "money", false, true, M, None),
            ];
            v.extend(void);
            v.push(notes);
            v
        }
        "sales_invoice_line" | "sales_allowance_line" => text_lines(()),
        "sales_invoice_shipment" => vec![(
            "shipment_id",
            "出貨單",
            "relation",
            true,
            false,
            None,
            Some("shipment"),
        )],
        "sales_allowance" => {
            let mut v = vec![
                (
                    "sales_invoice_id",
                    "原發票",
                    "relation",
                    true,
                    false,
                    None,
                    Some("sales_invoice"),
                ),
                (
                    "customer_id",
                    "客戶",
                    "relation",
                    false,
                    true,
                    None,
                    Some("customer"),
                ),
                (
                    "allowance_date",
                    "折讓日期",
                    "date",
                    true,
                    false,
                    None,
                    None,
                ),
                ("tax_period", "期別", "text", false, true, None, None),
                ("tax_type", "稅別", "select", false, true, None, None),
                (
                    "sales_return_id",
                    "銷貨退回",
                    "relation",
                    false,
                    false,
                    None,
                    Some("sales_return"),
                ),
                (
                    "tax_amount_manual",
                    "稅額（手動）",
                    "money",
                    false,
                    false,
                    M,
                    None,
                ),
                ("status", "狀態", "select", false, true, None, None),
                ("subtotal", "折讓未稅金額", "money", false, true, M, None),
                ("tax_amount", "折讓稅額", "money", false, true, M, None),
                ("total", "折讓總計", "money", false, true, M, None),
                ("reason", "折讓原因", "text", false, false, None, None),
            ];
            v.extend(void);
            v.push(notes);
            v
        }
        "purchase_invoice" => {
            let mut v = vec![
                ("doc_type", "憑證種類", "select", true, false, None, None),
                ("invoice_no", "憑證號碼", "text", true, false, None, None),
                ("invoice_date", "憑證日期", "date", true, false, None, None),
                ("tax_period", "期別", "text", false, true, None, None),
                (
                    "supplier_id",
                    "廠商",
                    "relation",
                    true,
                    false,
                    None,
                    Some("supplier"),
                ),
                (
                    "seller_tax_id",
                    "賣方統一編號",
                    "text",
                    false,
                    false,
                    None,
                    None,
                ),
                ("tax_type", "稅別", "select", true, false, None, None),
                ("subtotal", "未稅金額", "money", true, false, M, None),
                ("tax_amount", "稅額", "money", true, false, M, None),
                ("total", "總計", "money", false, true, M, None),
                ("deductible", "可扣抵", "boolean", true, false, None, None),
                ("status", "狀態", "select", false, true, None, None),
            ];
            v.extend(void);
            v.push(notes);
            v
        }
        "purchase_invoice_receipt" => vec![(
            "goods_receipt_id",
            "進貨單",
            "relation",
            true,
            false,
            None,
            Some("goods_receipt"),
        )],
        "purchase_allowance" => {
            let mut v = vec![
                (
                    "purchase_invoice_id",
                    "原憑證",
                    "relation",
                    true,
                    false,
                    None,
                    Some("purchase_invoice"),
                ),
                (
                    "supplier_id",
                    "廠商",
                    "relation",
                    false,
                    true,
                    None,
                    Some("supplier"),
                ),
                (
                    "allowance_date",
                    "折讓日期",
                    "date",
                    true,
                    false,
                    None,
                    None,
                ),
                ("tax_period", "期別", "text", false, true, None, None),
                ("tax_type", "稅別", "select", false, true, None, None),
                (
                    "purchase_return_id",
                    "進貨退回",
                    "relation",
                    false,
                    false,
                    None,
                    Some("purchase_return"),
                ),
                ("subtotal", "折讓未稅金額", "money", true, false, M, None),
                ("tax_amount", "折讓稅額", "money", true, false, M, None),
                ("total", "折讓總計", "money", false, true, M, None),
                ("status", "狀態", "select", false, true, None, None),
                ("reason", "折讓原因", "text", false, false, None, None),
            ];
            v.extend(void);
            v.push(notes);
            v
        }
        _ => unreachable!("{entity}"),
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

/// 系統欄位的定義與 §8.11 的表一致。
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
        let spec_keys: Vec<&str> = specs.iter().map(|s| s.0).collect();
        assert_eq!(keys, spec_keys, "{entity} 的欄位與順序");

        for (i, (row, spec)) in rows.iter().zip(&specs).enumerate() {
            let (key, label, data_type, required, readonly, decimals, relation) = *spec;
            let at = format!("{entity}.{key}");
            assert_eq!(row.label, label, "{at}");
            assert_eq!(row.data_type, data_type, "{at}");
            assert!(row.is_system && row.is_active && row.is_visible, "{at}");
            assert_eq!(row.storage, "column", "{at}");
            assert_eq!(row.column_name.as_deref(), Some(key), "{at}");
            assert_eq!(row.is_required, required, "{at} is_required");
            assert_eq!(row.is_readonly, readonly, "{at} is_readonly");
            assert_eq!(row.decimal_places, decimals, "{at}");
            assert_eq!(row.relation_key.as_deref(), relation, "{at}");
            assert_eq!(row.ui_widget, None, "{at}");
            assert!(!row.is_unique, "{at}");
            assert_eq!(
                row.is_searchable,
                matches!(key, "invoice_no" | "buyer_tax_id"),
                "{at} is_searchable"
            );
            assert_eq!(row.sort_order, (i as i32 + 1) * 10, "{at} sort_order");

            let expected_validation = match (key, readonly) {
                ("qty" | "unit_price" | "subtotal" | "tax_amount" | "tax_amount_manual", false) => {
                    Some(json!({ "min": "0" }))
                }
                ("description", _) => Some(json!({ "max_length": 200 })),
                _ => None,
            };
            assert_eq!(row.validation, expected_validation, "{at} validation");

            let expected_default = match key {
                "invoice_type" | "doc_type" => Some(json!("triplicate")),
                "tax_type" if entity == "purchase_invoice" => Some(json!("taxable")),
                "deductible" => Some(json!(true)),
                _ => None,
            };
            assert_eq!(row.default_value, expected_default, "{at} default_value");

            let options: Option<Vec<SelectOption>> = row
                .options
                .clone()
                .map(|v| serde_json::from_value(v).unwrap());
            let pairs: Option<Vec<(&str, &str)>> = options.as_ref().map(|o| {
                o.iter()
                    .map(|o| (o.value.as_str(), o.label.as_str()))
                    .collect()
            });
            let posted = match entity {
                "purchase_invoice" => "已登錄",
                _ => "已開立",
            };
            let expected_options = match key {
                "tax_type" => Some(vec![
                    ("taxable", "應稅"),
                    ("zero_rated", "零稅率"),
                    ("exempt", "免稅"),
                ]),
                "status" => Some(vec![
                    ("draft", "草稿"),
                    ("posted", posted),
                    ("void", "已作廢"),
                ]),
                "invoice_type" => Some(vec![("triplicate", "三聯式"), ("duplicate", "二聯式")]),
                "doc_type" => Some(vec![
                    ("triplicate", "三聯式發票"),
                    ("electronic", "電子發票"),
                    ("customs", "海關代徵營業稅繳納證"),
                ]),
                _ => None,
            };
            assert_eq!(pairs, expected_options, "{at} options");
        }
    }
}

/// 實體欄位：每個系統欄位都有同名的欄；不是欄位的值與共通欄位存在。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn table_columns(pool: PgPool) {
    let tables = [
        ("sales_invoice", "sales_invoices", Some("record_no")),
        (
            "sales_invoice_line",
            "sales_invoice_lines",
            Some("sales_invoice_id"),
        ),
        (
            "sales_invoice_shipment",
            "sales_invoice_shipments",
            Some("sales_invoice_id"),
        ),
        ("sales_allowance", "sales_allowances", Some("record_no")),
        (
            "sales_allowance_line",
            "sales_allowance_lines",
            Some("sales_allowance_id"),
        ),
        ("purchase_invoice", "purchase_invoices", Some("record_no")),
        (
            "purchase_invoice_receipt",
            "purchase_invoice_receipts",
            Some("purchase_invoice_id"),
        ),
        (
            "purchase_allowance",
            "purchase_allowances",
            Some("record_no"),
        ),
    ];
    for (entity, table, first) in tables {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name::text FROM information_schema.columns
             WHERE table_name = $1 ORDER BY ordinal_position",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(columns[0], "id", "{table}");
        assert_eq!(columns.get(1).map(String::as_str), first, "{table}");
        for (key, ..) in specs(entity) {
            assert!(columns.iter().any(|c| c == key), "{table}.{key}");
        }
        let tail = [
            "custom_data",
            "formula_errors",
            "created_at",
            "created_by",
            "updated_at",
            "updated_by",
            "deleted_at",
            "version",
        ];
        assert_eq!(&columns[columns.len() - tail.len()..], tail, "{table}");
        if first == Some("record_no") {
            for c in ["posted_at", "posted_by", "voided_at", "voided_by"] {
                assert!(columns.iter().any(|x| x == c), "{table}.{c}");
            }
        } else {
            assert_eq!(columns[2], "sort_order", "{table}");
        }
    }
}

// ---------------------------------------------------------------------------
// 限制
// ---------------------------------------------------------------------------

/// 銷項發票號碼：草稿不限制；開立後全域唯一，含已作廢。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn sales_invoice_no_unique_after_posting(pool: PgPool) {
    let (customer, _, user) = seed(&pool).await;
    let a = insert_sales_invoice(&pool, customer, "SI-1", "AB12345678").await;
    let b = insert_sales_invoice(&pool, customer, "SI-2", "AB12345678").await;

    set_status(&pool, "sales_invoices", a, "posted", user).await;
    let err = sqlx::query("UPDATE sales_invoices SET status = 'posted', posted_at = now(), posted_by = $2 WHERE id = $1")
        .bind(b)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        sqlstate(&err),
        ("23505".into(), Some("uq_sales_invoices_invoice_no".into()))
    );

    // 作廢後號碼仍不能再用
    set_status(&pool, "sales_invoices", a, "void", user).await;
    let err = sqlx::query("UPDATE sales_invoices SET status = 'posted', posted_at = now(), posted_by = $2 WHERE id = $1")
        .bind(b)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        sqlstate(&err).1.as_deref(),
        Some("uq_sales_invoices_invoice_no")
    );
}

/// 進項憑證號碼：同一廠商 + 號碼在已登錄中唯一；作廢後可以重新登錄；不同廠商不限制。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn purchase_invoice_no_unique_per_supplier(pool: PgPool) {
    let (_, supplier, user) = seed(&pool).await;
    let other: Uuid = sqlx::query_scalar(
        "INSERT INTO suppliers (id, code, name) VALUES ($1, 'S2', '另一家') RETURNING id",
    )
    .bind(Uuid::now_v7())
    .fetch_one(&pool)
    .await
    .unwrap();
    let a = insert_purchase_invoice(&pool, supplier, "PI-1", "CD12345678").await;
    let b = insert_purchase_invoice(&pool, supplier, "PI-2", "CD12345678").await;
    let c = insert_purchase_invoice(&pool, other, "PI-3", "CD12345678").await;

    set_status(&pool, "purchase_invoices", a, "posted", user).await;
    set_status(&pool, "purchase_invoices", c, "posted", user).await;
    let err = sqlx::query(
        "UPDATE purchase_invoices SET status = 'posted', posted_at = now(), posted_by = $2 WHERE id = $1",
    )
    .bind(b)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23505".into(),
            Some("uq_purchase_invoices_invoice_no".into())
        )
    );

    set_status(&pool, "purchase_invoices", a, "void", user).await;
    set_status(&pool, "purchase_invoices", b, "posted", user).await;
}

/// 作廢日期與原因只在已作廢時有值；發票種類、憑證種類、金額的 CHECK。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn document_checks(pool: PgPool) {
    let (customer, supplier, user) = seed(&pool).await;
    let invoice = insert_sales_invoice(&pool, customer, "SI-1", "AB12345678").await;

    let cases = [
        (
            "UPDATE sales_invoices SET void_date = '2026-11-20', void_reason = 'x' WHERE id = $1",
            "sales_invoices_void_date_check",
        ),
        (
            "UPDATE sales_invoices SET void_reason = 'x' WHERE id = $1",
            "sales_invoices_void_reason_check",
        ),
        (
            "UPDATE sales_invoices SET invoice_type = 'electronic' WHERE id = $1",
            "sales_invoices_invoice_type_check",
        ),
        (
            "UPDATE sales_invoices SET tax_amount_manual = -1 WHERE id = $1",
            "sales_invoices_tax_amount_manual_check",
        ),
    ];
    for (sql, constraint) in cases {
        let err = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(invoice)
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            sqlstate(&err),
            ("23514".into(), Some(constraint.into())),
            "{sql}"
        );
    }

    // 作廢時沒有作廢日期
    set_status(&pool, "sales_invoices", invoice, "posted", user).await;
    let err = sqlx::query(
        "UPDATE sales_invoices SET status = 'void', voided_at = now(), voided_by = $2 WHERE id = $1",
    )
    .bind(invoice)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        sqlstate(&err).1.as_deref(),
        Some("sales_invoices_void_date_check")
    );

    let pi = insert_purchase_invoice(&pool, supplier, "PI-1", "CD12345678").await;
    for (sql, constraint) in [
        (
            "UPDATE purchase_invoices SET doc_type = 'duplicate' WHERE id = $1",
            "purchase_invoices_doc_type_check",
        ),
        (
            "UPDATE purchase_invoices SET subtotal = -1 WHERE id = $1",
            "purchase_invoices_subtotal_check",
        ),
    ] {
        let err = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(pi)
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            sqlstate(&err),
            ("23514".into(), Some(constraint.into())),
            "{sql}"
        );
    }
}

// ---------------------------------------------------------------------------
// 存取函式
// ---------------------------------------------------------------------------

/// 折讓帶入表頭用的原發票（憑證）：已刪除、不存在時為 `None`。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn invoice_access_for_allowance(pool: PgPool) {
    let (customer, supplier, _) = seed(&pool).await;
    let si = insert_sales_invoice(&pool, customer, "SI-1", "AB12345678").await;
    let pi = insert_purchase_invoice(&pool, supplier, "PI-1", "CD12345678").await;

    let mut tx = pool.begin().await.unwrap();
    let found = invoicing::find_sales_invoice_for_allowance(&mut *tx, si)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.party_id, customer);
    assert_eq!(found.status, "draft");
    assert_eq!(found.tax_type, "taxable");
    assert_eq!(found.doc_type, "triplicate");
    let found = invoicing::find_purchase_invoice_for_allowance(&mut *tx, pi)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.party_id, supplier);
    assert_eq!(found.tax_type, "zero_rated");
    assert!(
        invoicing::find_sales_invoice_for_allowance(&mut *tx, pi)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();

    sqlx::query("UPDATE sales_invoices SET deleted_at = now() WHERE id = $1")
        .bind(si)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        invoicing::find_sales_invoice_for_allowance(&pool, si)
            .await
            .unwrap()
            .is_none()
    );
}
