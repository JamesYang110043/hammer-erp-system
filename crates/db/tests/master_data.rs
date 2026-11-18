//! 0010_master_data 的整合測試：M3 基本資料表、初始資料、系統資料類別與系統欄位的登記
//! （docs/architecture.md §8.6、§8.7）。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use hammer_db::sqlx::{self, PgPool, migrate::Migrator};
use hammer_shared::schema::{SelectOption, Validation};
use serde_json::{Value, json};
use uuid::Uuid;

/// 0010 之前的最後一個 migration 版本。
const BEFORE_MASTER_DATA: i64 = 9;

/// 系統資料表：資料類別代碼、資料表、明細的父資料欄位。
const SYSTEM_TABLES: &[(&str, &str, Option<&str>)] = &[
    ("product", "products", None),
    ("product_category", "product_categories", None),
    ("unit", "units", None),
    ("customer", "customers", None),
    ("customer_contact", "customer_contacts", Some("customer_id")),
    ("supplier", "suppliers", None),
    ("supplier_contact", "supplier_contacts", Some("supplier_id")),
];

/// M3 的系統資料表名稱（之後的 migration 另外登記的系統資料類別不在此檔檢查）。
fn m3_tables() -> Vec<&'static str> {
    SYSTEM_TABLES.iter().map(|(_, table, _)| *table).collect()
}

/// 每張系統資料表都有、但不是欄位的實體欄位（§8.6「系統資料表的共同規則」）。
const NON_FIELD_COLUMNS: &[&str] = &[
    "id",
    "custom_data",
    "formula_errors",
    "created_at",
    "created_by",
    "updated_at",
    "updated_by",
    "deleted_at",
    "version",
];

/// architecture.md §8.6、§8.7 的一個系統欄位。
struct Spec {
    key: &'static str,
    label: &'static str,
    data_type: &'static str,
    required: bool,
    unique: bool,
    searchable: bool,
    decimal_places: Option<i16>,
    default_value: Option<Value>,
    validation: Option<Value>,
    relation: Option<&'static str>,
    ui_widget: Option<&'static str>,
    options: Option<Value>,
}

fn spec(key: &'static str, label: &'static str, data_type: &'static str) -> Spec {
    Spec {
        key,
        label,
        data_type,
        required: false,
        unique: false,
        searchable: false,
        decimal_places: None,
        default_value: None,
        validation: None,
        relation: None,
        ui_widget: None,
        options: None,
    }
}

impl Spec {
    fn required(mut self) -> Self {
        self.required = true;
        self
    }
    fn searchable(mut self) -> Self {
        self.searchable = true;
        self
    }
    /// 必填、唯一、`max_length 50` 的代碼。
    fn code(mut self) -> Self {
        self.required = true;
        self.unique = true;
        self.validation = Some(json!({ "max_length": 50 }));
        self
    }
    fn decimals(mut self, n: i16) -> Self {
        self.decimal_places = Some(n);
        self
    }
    fn min0(mut self) -> Self {
        self.validation = Some(json!({ "min": "0" }));
        self
    }
    fn default(mut self, v: Value) -> Self {
        self.default_value = Some(v);
        self
    }
    fn to(mut self, entity: &'static str) -> Self {
        self.relation = Some(entity);
        self
    }
    fn widget(mut self, w: &'static str) -> Self {
        self.ui_widget = Some(w);
        self
    }
    fn options(mut self, v: Value) -> Self {
        self.options = Some(v);
        self
    }
}

fn party_fields(code_label: &'static str, supplier: bool) -> Vec<Spec> {
    let mut fields = vec![
        spec("code", code_label, "text").code(),
        spec("name", "公司名稱", "text").required().searchable(),
        spec("short_name", "簡稱", "text").searchable(),
        spec("tax_id", "統一編號", "text").searchable(),
        spec("phone", "電話", "text"),
        spec("fax", "傳真", "text"),
        spec("email", "Email", "text"),
        spec("address", "聯絡地址", "text"),
        spec("invoice_title", "發票抬頭", "text"),
        spec("invoice_address", "發票地址", "text"),
        spec("payment_terms_days", "付款天數", "integer").min0(),
        spec("credit_limit", "信用額度", "money").decimals(2).min0(),
    ];
    if supplier {
        fields.extend([
            spec("default_currency_code", "預設幣別", "text")
                .required()
                .default(json!("TWD"))
                .widget("currency"),
            spec("country", "國家", "text"),
            spec("bank_info", "匯款資訊", "long_text"),
        ]);
    }
    fields.extend([
        spec("is_active", "啟用", "boolean")
            .required()
            .default(json!(true)),
        spec("notes", "備註", "long_text"),
    ]);
    fields
}

fn contact_fields() -> Vec<Spec> {
    vec![
        spec("name", "姓名", "text").required(),
        spec("title", "職稱", "text"),
        spec("phone", "電話", "text"),
        spec("mobile", "手機", "text"),
        spec("email", "Email", "text"),
        spec("is_primary", "主要聯絡人", "boolean")
            .required()
            .default(json!(false)),
        spec("notes", "備註", "long_text"),
    ]
}

/// 各系統資料類別的系統欄位，依 §8.6、§8.7 表中順序。
fn spec_fields(entity: &str) -> Vec<Spec> {
    match entity {
        "unit" => vec![
            spec("code", "代碼", "text").code(),
            spec("name", "名稱", "text").required().searchable(),
        ],
        "product_category" => vec![
            spec("name", "名稱", "text").required().searchable(),
            spec("parent_id", "上層分類", "relation").to("product_category"),
            spec("sort_order", "排序", "integer")
                .required()
                .default(json!(0)),
        ],
        "product" => vec![
            spec("code", "料號", "text").code(),
            spec("name", "品名", "text").required().searchable(),
            spec("spec", "規格", "text"),
            spec("product_type", "類型", "select")
                .required()
                .default(json!("goods"))
                .options(json!([
                    { "value": "goods", "label": "實體商品" },
                    { "value": "service", "label": "服務" },
                ])),
            spec("category_id", "分類", "relation").to("product_category"),
            spec("unit_id", "單位", "relation").required().to("unit"),
            spec("barcode", "條碼", "text").searchable(),
            spec("tax_type", "稅別", "select")
                .required()
                .default(json!("taxable"))
                .options(json!([
                    { "value": "taxable", "label": "應稅" },
                    { "value": "zero_rated", "label": "零稅率" },
                    { "value": "exempt", "label": "免稅" },
                ])),
            spec("default_sale_price", "預設售價", "money")
                .decimals(4)
                .min0(),
            spec("safety_stock", "安全庫存", "decimal")
                .decimals(4)
                .min0(),
            spec("is_active", "啟用", "boolean")
                .required()
                .default(json!(true)),
            spec("notes", "備註", "long_text"),
        ],
        "customer" => party_fields("客戶編號", false),
        "supplier" => party_fields("廠商編號", true),
        "customer_contact" | "supplier_contact" => contact_fields(),
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

async fn fields_of(pool: &PgPool, entity_key: &str) -> Vec<FieldRow> {
    sqlx::query_as(
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
    .bind(entity_key)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct ColumnRow {
    column_name: String,
    data_type: String,
    is_nullable: String,
    numeric_precision: Option<i32>,
    numeric_scale: Option<i32>,
}

async fn columns_of(pool: &PgPool, table: &str) -> Vec<ColumnRow> {
    sqlx::query_as(
        "SELECT column_name::text, data_type::text, is_nullable::text,
                numeric_precision::int, numeric_scale::int
         FROM information_schema.columns
         WHERE table_schema = current_schema() AND table_name = $1
         ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn index_defs(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE schemaname = current_schema() AND tablename = $1",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// 取出資料庫錯誤的 SQLSTATE 與約束名稱。
fn db_error(err: &sqlx::Error) -> (String, Option<String>) {
    let db = err.as_database_error().expect("應為資料庫錯誤");
    (
        db.code().unwrap().into_owned(),
        db.constraint().map(str::to_owned),
    )
}

async fn exec(pool: &PgPool, sql: &'static str) -> Result<(), sqlx::Error> {
    sqlx::query(sql).execute(pool).await.map(|_| ())
}

async fn insert_unit(pool: &PgPool, code: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO units (id, code, name) VALUES ($1, $2, $2)")
        .bind(id)
        .bind(code)
        .execute(pool)
        .await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// migration
// ---------------------------------------------------------------------------

/// 在已有資料（0009 之前的 schema）的資料庫上套用 0010：既有資料保留，系統資料類別寫入。
#[sqlx::test(migrations = false)]
async fn applies_on_existing_database(pool: PgPool) {
    let before = Migrator::with_migrations(
        hammer_db::MIGRATOR
            .iter()
            .filter(|m| m.version <= BEFORE_MASTER_DATA)
            .cloned()
            .collect(),
    );
    before.run(&pool).await.unwrap();

    let entity = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entity_definitions (id, key, label, kind, storage)
         VALUES ($1, 'repair_order', '維修單', 'master', 'custom_records')",
    )
    .bind(entity)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO custom_records (id, entity_id, data) VALUES ($1, $2, '{\"note\":\"a\"}')",
    )
    .bind(Uuid::now_v7())
    .bind(entity)
    .execute(&pool)
    .await
    .unwrap();

    hammer_db::run_migrations(&pool).await.unwrap();
    // 重複執行不會出錯
    hammer_db::run_migrations(&pool).await.unwrap();

    // 之後的 migration（例如 0011 的盤點調整單）另外登記的系統資料類別不在此檢查
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM entity_definitions
         WHERE NOT is_system OR table_name = ANY($1)
         ORDER BY is_system, key",
    )
    .bind(m3_tables())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        keys,
        [
            "repair_order",
            "customer",
            "customer_contact",
            "product",
            "product_category",
            "supplier",
            "supplier_contact",
            "unit",
        ]
    );
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM custom_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(records, 1);
}

// ---------------------------------------------------------------------------
// 初始資料
// ---------------------------------------------------------------------------

/// 預設 6 種幣別，只有 TWD 是本位幣；本位幣只能有一個、代碼必須是 3 個大寫英文字母。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn default_currencies(pool: PgPool) {
    let rows: Vec<(String, String, Option<String>, i16, bool)> = sqlx::query_as(
        "SELECT code, name, symbol, decimal_places, is_base
         FROM currencies ORDER BY is_base DESC, code",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let rows: Vec<(&str, &str, Option<&str>, i16, bool)> = rows
        .iter()
        .map(|(c, n, s, d, b)| (c.as_str(), n.as_str(), s.as_deref(), *d, *b))
        .collect();
    assert_eq!(
        rows,
        [
            ("TWD", "新台幣", Some("NT$"), 0, true),
            ("CNY", "人民幣", Some("CN¥"), 2, false),
            ("EUR", "歐元", Some("€"), 2, false),
            ("HKD", "港幣", Some("HK$"), 2, false),
            ("JPY", "日圓", Some("JP¥"), 0, false),
            ("USD", "美元", Some("US$"), 2, false),
        ]
    );
    let not_pristine: i64 =
        sqlx::query_scalar("SELECT count(*) FROM currencies WHERE NOT is_active OR version <> 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(not_pristine, 0, "預設幣別都應為啟用、版本 1");

    let err = exec(
        &pool,
        "UPDATE currencies SET is_base = true WHERE code = 'USD'",
    )
    .await
    .unwrap_err();
    assert_eq!(
        db_error(&err),
        ("23505".to_owned(), Some("uq_currencies_is_base".to_owned()))
    );

    for (code, dp) in [("usd", 2), ("US", 2), ("ABCD", 2), ("GBP", 5)] {
        let err = sqlx::query(
            "INSERT INTO currencies (id, code, name, decimal_places) VALUES ($1, $2, 'x', $3)",
        )
        .bind(Uuid::now_v7())
        .bind(code)
        .bind(dp)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(db_error(&err).0, "23514", "{code} / {dp}");
    }
}

/// 預設 3 種稅別。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn default_tax_rates(pool: PgPool) {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT code, name, rate::text FROM tax_rates ORDER BY rate DESC, code")
            .fetch_all(&pool)
            .await
            .unwrap();
    let rows: Vec<(&str, &str, &str)> = rows
        .iter()
        .map(|(c, n, r)| (c.as_str(), n.as_str(), r.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("taxable", "應稅", "0.0500"),
            ("exempt", "免稅", "0.0000"),
            ("zero_rated", "零稅率", "0.0000"),
        ]
    );
}

// ---------------------------------------------------------------------------
// 系統資料類別與系統欄位
// ---------------------------------------------------------------------------

/// M3 的 7 個系統資料類別與 §8.6「M3 的系統資料類別」一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn system_entities_are_registered(pool: PgPool) {
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
                e.title_template, e.icon, e.numbering_pattern, e.sort_order, e.is_active
         FROM entity_definitions e
         LEFT JOIN entity_definitions p ON p.id = e.parent_entity_id
         WHERE e.table_name = ANY($1)
         ORDER BY e.id",
    )
    .bind(m3_tables())
    .fetch_all(&pool)
    .await
    .unwrap();

    let expected = [
        (
            "product",
            "商品",
            "master",
            "products",
            None,
            "{code} {name}",
            10,
        ),
        (
            "product_category",
            "商品分類",
            "master",
            "product_categories",
            None,
            "{name}",
            20,
        ),
        ("unit", "單位", "master", "units", None, "{name}", 30),
        (
            "customer",
            "客戶",
            "master",
            "customers",
            None,
            "{code} {name}",
            40,
        ),
        (
            "customer_contact",
            "客戶聯絡人",
            "detail",
            "customer_contacts",
            Some("customer"),
            "{name}",
            0,
        ),
        (
            "supplier",
            "廠商",
            "master",
            "suppliers",
            None,
            "{code} {name}",
            50,
        ),
        (
            "supplier_contact",
            "廠商聯絡人",
            "detail",
            "supplier_contacts",
            Some("supplier"),
            "{name}",
            0,
        ),
    ];
    assert_eq!(rows.len(), expected.len());
    for (row, (key, label, kind, table, parent, title, sort)) in rows.iter().zip(expected) {
        assert_eq!(row.0, key);
        assert_eq!(row.1, label, "{key}");
        assert_eq!(row.2, kind, "{key}");
        assert!(row.3, "{key} 應為系統資料類別");
        assert_eq!(row.4, "table", "{key}");
        assert_eq!(row.5.as_deref(), Some(table), "{key}");
        assert_eq!(row.6.as_deref(), parent, "{key}");
        assert_eq!(row.7.as_deref(), Some(title), "{key}");
        assert_eq!(row.8, None, "{key} 的 icon");
        assert_eq!(row.9, None, "{key} 的 numbering_pattern");
        assert_eq!(row.10, sort, "{key}");
        assert!(row.11, "{key} 應為啟用");
    }
}

/// 系統欄位的定義與 §8.6、§8.7 的表一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn system_fields_match_spec(pool: PgPool) {
    for (entity, _, _) in SYSTEM_TABLES {
        let rows = fields_of(&pool, entity).await;
        let specs = spec_fields(entity);
        let keys: Vec<&str> = rows.iter().map(|r| r.field_key.as_str()).collect();
        let spec_keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
        assert_eq!(keys, spec_keys, "{entity} 的欄位與順序");

        for (i, (row, spec)) in rows.iter().zip(&specs).enumerate() {
            let at = format!("{entity}.{}", spec.key);
            assert_eq!(row.label, spec.label, "{at}");
            assert_eq!(row.data_type, spec.data_type, "{at}");
            assert!(row.is_system, "{at}");
            assert_eq!(row.storage, "column", "{at}");
            assert_eq!(row.column_name.as_deref(), Some(spec.key), "{at}");
            assert_eq!(row.is_required, spec.required, "{at} is_required");
            assert!(row.is_visible && row.is_active && !row.is_readonly, "{at}");
            assert_eq!(row.is_unique, spec.unique, "{at} is_unique");
            assert_eq!(row.is_searchable, spec.searchable, "{at} is_searchable");
            assert_eq!(row.decimal_places, spec.decimal_places, "{at}");
            assert_eq!(row.default_value, spec.default_value, "{at} default_value");
            assert_eq!(row.validation, spec.validation, "{at} validation");
            assert_eq!(row.options, spec.options, "{at} options");
            assert_eq!(row.relation_key.as_deref(), spec.relation, "{at}");
            assert_eq!(row.ui_widget.as_deref(), spec.ui_widget, "{at}");
            assert_eq!(row.sort_order, (i as i32 + 1) * 10, "{at} sort_order");

            // JSONB 的格式能被 hammer_shared 讀取
            if let Some(v) = &row.validation {
                serde_json::from_value::<Validation>(v.clone()).unwrap();
            }
            if let Some(v) = &row.options {
                serde_json::from_value::<Vec<SelectOption>>(v.clone()).unwrap();
            }
        }
    }
}

/// 實體欄位與欄位定義一致：每個系統欄位都有同名的實體欄位，型態、小數位數、可否為空相符；
/// 除了共同欄位與明細的父資料欄位、順序之外，沒有未登記的實體欄位。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn table_columns_match_field_definitions(pool: PgPool) {
    for (entity, table, parent) in SYSTEM_TABLES {
        let columns = columns_of(&pool, table).await;
        let specs = spec_fields(entity);

        let mut expected: Vec<&str> = vec!["id"];
        if let Some(parent) = parent {
            expected.extend([*parent, "sort_order"]);
        }
        expected.extend(specs.iter().map(|s| s.key));
        expected.extend(&NON_FIELD_COLUMNS[1..]);
        let names: Vec<&str> = columns.iter().map(|c| c.column_name.as_str()).collect();
        assert_eq!(names, expected, "{table} 的欄位與順序");

        for spec in &specs {
            let col = columns.iter().find(|c| c.column_name == spec.key).unwrap();
            let at = format!("{table}.{}", spec.key);
            let sql_type = match spec.data_type {
                "text" | "long_text" | "select" => "text",
                "integer" => "integer",
                "decimal" | "money" => "numeric",
                "boolean" => "boolean",
                "relation" => "uuid",
                other => panic!("{at}：未預期的型態 {other}"),
            };
            assert_eq!(col.data_type, sql_type, "{at}");
            if sql_type == "numeric" {
                assert_eq!(col.numeric_precision, Some(18), "{at}");
                assert_eq!(
                    col.numeric_scale,
                    spec.decimal_places.map(i32::from),
                    "{at}"
                );
            }
            assert_eq!(col.is_nullable == "NO", spec.required, "{at} NOT NULL");
        }
    }
}

/// 每張系統資料表都有 `custom_data` 的 GIN 索引、`set_updated_at` trigger；
/// 唯一欄位有 `lower()` 的部分唯一索引，可搜尋的欄位有 `lower()` 索引，外鍵欄位有索引。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn system_tables_have_required_indexes_and_triggers(pool: PgPool) {
    for (entity, table, parent) in SYSTEM_TABLES {
        let defs = index_defs(&pool, table).await;
        let has = |needle: &str| defs.iter().any(|d| d.contains(needle));
        assert!(has("USING gin (custom_data)"), "{table}: {defs:?}");

        for spec in spec_fields(entity) {
            if spec.unique {
                assert!(
                    defs.iter().any(|d| d.starts_with("CREATE UNIQUE INDEX")
                        && d.contains(&format!("(lower({}))", spec.key))
                        && d.contains("WHERE (deleted_at IS NULL)")),
                    "{table}.{} 的唯一索引：{defs:?}",
                    spec.key
                );
            } else if spec.searchable {
                assert!(
                    has(&format!("(lower({}))", spec.key)),
                    "{table}.{} 的搜尋索引：{defs:?}",
                    spec.key
                );
            }
            if spec.relation.is_some() {
                assert!(
                    has(&format!("btree ({})", spec.key)),
                    "{table}.{} 的外鍵索引：{defs:?}",
                    spec.key
                );
            }
        }
        if let Some(parent) = parent {
            assert!(
                has(&format!(
                    "btree ({parent}, sort_order, id) WHERE (deleted_at IS NULL)"
                )),
                "{table}: {defs:?}"
            );
        }

        let trigger: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid
                 WHERE t.tgrelid = $1::regclass AND p.proname = 'set_updated_at'
             )",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(trigger, "{table} 沒有 set_updated_at trigger");
    }
}

// ---------------------------------------------------------------------------
// 限制
// ---------------------------------------------------------------------------

/// `code` 不分大小寫不可重複，只比對未刪除的資料；刪除後代碼可以再使用。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn code_is_unique_case_insensitively_among_undeleted(pool: PgPool) {
    let first = insert_unit(&pool, "PCS").await.unwrap();
    let err = insert_unit(&pool, "pcs").await.unwrap_err();
    assert_eq!(
        db_error(&err),
        ("23505".to_owned(), Some("uq_units_code".to_owned()))
    );

    sqlx::query("UPDATE units SET deleted_at = now() WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    insert_unit(&pool, "pcs").await.unwrap();
}

/// 列舉值、0 以上、JSONB 物件、必填欄位的 CHECK 與 NOT NULL；廠商的預設幣別為 TWD 且必須存在。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn checks_and_defaults(pool: PgPool) {
    let unit = insert_unit(&pool, "pcs").await.unwrap();
    let insert_product = |extra_cols: &'static str, extra_vals: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO products (id, code, name, unit_id{extra_cols})
                 VALUES ($1, $2, 'x', $3{extra_vals})"
            )))
            .bind(Uuid::now_v7())
            .bind(Uuid::now_v7().to_string())
            .bind(unit)
            .execute(&pool)
            .await
            .map(|_| ())
        }
    };

    // 預設值
    insert_product("", "").await.unwrap();
    let (product_type, tax_type, is_active): (String, String, bool) =
        sqlx::query_as("SELECT product_type, tax_type, is_active FROM products")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (product_type.as_str(), tax_type.as_str(), is_active),
        ("goods", "taxable", true)
    );

    for (cols, vals) in [
        (", product_type", ", 'bundle'"),
        (", tax_type", ", 'vat'"),
        (", default_sale_price", ", -0.0001"),
        (", safety_stock", ", -1"),
        (", custom_data", ", '[]'"),
        (", formula_errors", ", '\"x\"'"),
    ] {
        let err = insert_product(cols, vals).await.unwrap_err();
        assert_eq!(db_error(&err).0, "23514", "{cols} = {vals}");
    }
    // 單位必填、必須存在
    let err = exec(
        &pool,
        "INSERT INTO products (id, code, name, unit_id) VALUES (gen_random_uuid(), 'n', 'x', NULL)",
    )
    .await
    .unwrap_err();
    assert_eq!(db_error(&err).0, "23502");
    let err = exec(
        &pool,
        "INSERT INTO products (id, code, name, unit_id)
         VALUES (gen_random_uuid(), 'n', 'x', gen_random_uuid())",
    )
    .await
    .unwrap_err();
    assert_eq!(db_error(&err).0, "23503");

    for sql in [
        "INSERT INTO customers (id, code, name, payment_terms_days)
         VALUES (gen_random_uuid(), 'c1', 'x', -1)",
        "INSERT INTO customers (id, code, name, credit_limit)
         VALUES (gen_random_uuid(), 'c2', 'x', -0.01)",
        "INSERT INTO exchange_rates (id, currency_code, rate_date, rate)
         VALUES (gen_random_uuid(), 'USD', '2026-11-17', 0)",
    ] {
        let err = exec(&pool, sql).await.unwrap_err();
        assert_eq!(db_error(&err).0, "23514", "{sql}");
    }

    // 廠商的預設幣別
    let supplier = Uuid::now_v7();
    sqlx::query("INSERT INTO suppliers (id, code, name) VALUES ($1, 's1', 'x')")
        .bind(supplier)
        .execute(&pool)
        .await
        .unwrap();
    let currency: String =
        sqlx::query_scalar("SELECT default_currency_code FROM suppliers WHERE id = $1")
            .bind(supplier)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(currency, "TWD");
    let err = exec(
        &pool,
        "INSERT INTO suppliers (id, code, name, default_currency_code)
         VALUES (gen_random_uuid(), 's2', 'x', 'XXX')",
    )
    .await
    .unwrap_err();
    assert_eq!(db_error(&err).0, "23503");

    // 聯絡人必須屬於存在的廠商；主要聯絡人預設 false
    let err = exec(
        &pool,
        "INSERT INTO supplier_contacts (id, supplier_id, name)
         VALUES (gen_random_uuid(), gen_random_uuid(), 'x')",
    )
    .await
    .unwrap_err();
    assert_eq!(db_error(&err).0, "23503");
    sqlx::query("INSERT INTO supplier_contacts (id, supplier_id, name) VALUES ($1, $2, '王')")
        .bind(Uuid::now_v7())
        .bind(supplier)
        .execute(&pool)
        .await
        .unwrap();
    let (is_primary, sort_order): (bool, i32) =
        sqlx::query_as("SELECT is_primary, sort_order FROM supplier_contacts")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((is_primary, sort_order), (false, 0));
}

/// 同一幣別同一天只能有一筆未刪除的匯率；刪除後可以重新輸入；匯率保留 6 位小數。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn exchange_rate_is_unique_per_currency_and_day(pool: PgPool) {
    let insert = |rate: &'static str| {
        let pool = pool.clone();
        async move {
            let id = Uuid::now_v7();
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO exchange_rates (id, currency_code, rate_date, rate)
                 VALUES ($1, 'USD', '2026-11-17', {rate})"
            )))
            .bind(id)
            .execute(&pool)
            .await
            .map(|_| id)
        }
    };
    let first = insert("32.123456").await.unwrap();
    let err = insert("32.5").await.unwrap_err();
    assert_eq!(
        db_error(&err),
        (
            "23505".to_owned(),
            Some("uq_exchange_rates_currency_date".to_owned())
        )
    );
    // 其他幣別同一天不受影響
    exec(
        &pool,
        "INSERT INTO exchange_rates (id, currency_code, rate_date, rate)
         VALUES (gen_random_uuid(), 'JPY', '2026-11-17', 0.21)",
    )
    .await
    .unwrap();

    sqlx::query("UPDATE exchange_rates SET deleted_at = now() WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    insert("32.5").await.unwrap();

    let rates: BTreeSet<String> = sqlx::query_scalar("SELECT rate::text FROM exchange_rates")
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        rates,
        BTreeSet::from(["0.210000".into(), "32.123456".into(), "32.500000".into()])
    );
}
