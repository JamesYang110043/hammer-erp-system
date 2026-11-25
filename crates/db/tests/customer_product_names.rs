//! 0017_customer_product_names 的整合測試：客戶品名資料表、資料類別與欄位定義的登記，
//! 以及銷貨明細 `product_name` 的補值（docs/architecture.md §8.7、§8.9「明細的品名」）。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use hammer_db::sqlx::{self, PgPool, migrate::Migrator};
use serde_json::{Value, json};
use uuid::Uuid;

/// 0017 之前的最後一個 migration 版本。
const BEFORE_CUSTOMER_PRODUCT_NAMES: i64 = 16;

fn sqlstate(err: &sqlx::Error) -> (String, Option<String>) {
    let db = err.as_database_error().expect("應為資料庫錯誤");
    (
        db.code().unwrap().into_owned(),
        db.constraint().map(str::to_owned),
    )
}

/// 客戶兩位、商品兩個，回傳 `(customers, products)`。
async fn seed(pool: &PgPool) -> ([Uuid; 2], [Uuid; 2]) {
    let customers = [Uuid::now_v7(), Uuid::now_v7()];
    for (i, id) in customers.iter().enumerate() {
        sqlx::query("INSERT INTO customers (id, code, name) VALUES ($1, $2, '客戶')")
            .bind(id)
            .bind(format!("C{i}"))
            .execute(pool)
            .await
            .unwrap();
    }
    let unit = Uuid::now_v7();
    sqlx::query("INSERT INTO units (id, code, name) VALUES ($1, 'pcs', '個')")
        .bind(unit)
        .execute(pool)
        .await
        .unwrap();
    let products = [Uuid::now_v7(), Uuid::now_v7()];
    for (i, id) in products.iter().enumerate() {
        sqlx::query("INSERT INTO products (id, code, name, unit_id) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(format!("P{i}"))
            .bind(format!("扳手 {i}"))
            .bind(unit)
            .execute(pool)
            .await
            .unwrap();
    }
    (customers, products)
}

async fn insert_name(
    pool: &PgPool,
    customer: Uuid,
    product: Uuid,
    name: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO customer_product_names (id, customer_id, product_id, name)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(customer)
    .bind(product)
    .bind(name)
    .execute(pool)
    .await
    .map(|_| id)
}

// ---------------------------------------------------------------------------
// migration
// ---------------------------------------------------------------------------

/// 在既有資料庫上套用 0017：既有的訂單、出貨、銷貨退回明細以商品名稱補上 `product_name`。
#[sqlx::test(migrations = false)]
async fn applies_on_existing_database(pool: PgPool) {
    let before = Migrator::with_migrations(
        hammer_db::MIGRATOR
            .iter()
            .filter(|m| m.version <= BEFORE_CUSTOMER_PRODUCT_NAMES)
            .cloned()
            .collect(),
    );
    before.run(&pool).await.unwrap();
    let ([customer, _], [p0, p1]) = seed(&pool).await;

    let order = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales_orders (id, record_no, customer_id, order_date, tax_type)
         VALUES ($1, 'SO-1', $2, '2026-11-01', 'taxable')",
    )
    .bind(order)
    .bind(customer)
    .execute(&pool)
    .await
    .unwrap();
    let order_line = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales_order_lines (id, sales_order_id, product_id, qty, unit_price)
         VALUES ($1, $2, $3, 1, 10)",
    )
    .bind(order_line)
    .bind(order)
    .bind(p0)
    .execute(&pool)
    .await
    .unwrap();

    let shipment = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO shipments (id, record_no, customer_id, ship_date, tax_type)
         VALUES ($1, 'SH-1', $2, '2026-11-02', 'taxable')",
    )
    .bind(shipment)
    .bind(customer)
    .execute(&pool)
    .await
    .unwrap();
    // 已刪除的明細也要補上
    let shipment_lines = [Uuid::now_v7(), Uuid::now_v7()];
    for (line, product, deleted) in [
        (shipment_lines[0], p0, false),
        (shipment_lines[1], p1, true),
    ] {
        sqlx::query(
            "INSERT INTO shipment_lines (id, shipment_id, product_id, qty, unit_price, deleted_at)
             VALUES ($1, $2, $3, 1, 10, CASE WHEN $4 THEN now() END)",
        )
        .bind(line)
        .bind(shipment)
        .bind(product)
        .bind(deleted)
        .execute(&pool)
        .await
        .unwrap();
    }

    let ret = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales_returns (id, record_no, shipment_id, customer_id, return_date, tax_type, tax_included)
         VALUES ($1, 'SR-1', $2, $3, '2026-11-03', 'taxable', false)",
    )
    .bind(ret)
    .bind(shipment)
    .bind(customer)
    .execute(&pool)
    .await
    .unwrap();
    let return_line = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales_return_lines (id, sales_return_id, shipment_line_id, product_id, qty, unit_price)
         VALUES ($1, $2, $3, $4, 1, 10)",
    )
    .bind(return_line)
    .bind(ret)
    .bind(shipment_lines[0])
    .bind(p0)
    .execute(&pool)
    .await
    .unwrap();

    hammer_db::run_migrations(&pool).await.unwrap();
    hammer_db::run_migrations(&pool).await.unwrap();

    for (table, id, expected) in [
        ("sales_order_lines", order_line, "扳手 0"),
        ("shipment_lines", shipment_lines[0], "扳手 0"),
        ("shipment_lines", shipment_lines[1], "扳手 1"),
        ("sales_return_lines", return_line, "扳手 0"),
    ] {
        let name: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT product_name FROM {table} WHERE id = $1"
        )))
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(name, expected, "{table}");
    }
}

// ---------------------------------------------------------------------------
// 資料類別與欄位定義
// ---------------------------------------------------------------------------

/// 客戶品名的資料類別與 §8.7 的表一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn entity_is_registered(pool: PgPool) {
    #[allow(clippy::type_complexity)]
    let row: (
        String,
        String,
        bool,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i32,
        bool,
    ) = sqlx::query_as(
        "SELECT e.label, e.kind, e.is_system, e.storage, e.table_name, p.key, e.title_template,
                e.numbering_pattern, e.sort_order, e.is_active
         FROM entity_definitions e
         LEFT JOIN entity_definitions p ON p.id = e.parent_entity_id
         WHERE e.key = 'customer_product_name'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "客戶品名".to_owned(),
            "detail".to_owned(),
            true,
            "table".to_owned(),
            Some("customer_product_names".to_owned()),
            Some("customer".to_owned()),
            Some("{product_id.code} {name}".to_owned()),
            None,
            10,
            true,
        )
    );

    // 客戶的明細子表依 sort_order：聯絡人 → 客戶品名
    let details: Vec<String> = sqlx::query_scalar(
        "SELECT e.key FROM entity_definitions e
         JOIN entity_definitions p ON p.id = e.parent_entity_id
         WHERE p.key = 'customer' ORDER BY e.sort_order, e.key",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(details, ["customer_contact", "customer_product_name"]);
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct FieldRow {
    field_key: String,
    label: String,
    data_type: String,
    column_name: Option<String>,
    is_required: bool,
    is_visible: bool,
    is_readonly: bool,
    is_searchable: bool,
    is_unique: bool,
    validation: Option<Value>,
    relation_key: Option<String>,
    sort_order: i32,
}

async fn fields_of(pool: &PgPool, entity: &str) -> Vec<FieldRow> {
    sqlx::query_as(
        "SELECT f.field_key, f.label, f.data_type, f.column_name, f.is_required, f.is_visible,
                f.is_readonly, f.is_searchable, f.is_unique, f.validation,
                r.key AS relation_key, f.sort_order
         FROM field_definitions f
         JOIN entity_definitions e ON e.id = f.entity_id
         LEFT JOIN entity_definitions r ON r.id = f.relation_entity_id
         WHERE e.key = $1 AND f.is_system AND f.is_active AND f.storage = 'column'
         ORDER BY f.sort_order",
    )
    .bind(entity)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn field(
    key: &str,
    label: &str,
    data_type: &str,
    required: bool,
    readonly: bool,
    searchable: bool,
    validation: Option<Value>,
    relation: Option<&str>,
    sort_order: i32,
) -> FieldRow {
    FieldRow {
        field_key: key.to_owned(),
        label: label.to_owned(),
        data_type: data_type.to_owned(),
        column_name: Some(key.to_owned()),
        is_required: required,
        is_visible: true,
        is_readonly: readonly,
        is_searchable: searchable,
        is_unique: false,
        validation,
        relation_key: relation.map(str::to_owned),
        sort_order,
    }
}

/// 客戶品名的系統欄位與 §8.7 一致。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn customer_product_name_fields_match_spec(pool: PgPool) {
    let max200 = Some(json!({ "max_length": 200 }));
    assert_eq!(
        fields_of(&pool, "customer_product_name").await,
        vec![
            field(
                "product_id",
                "商品",
                "relation",
                true,
                false,
                false,
                None,
                Some("product"),
                10
            ),
            field(
                "name",
                "客戶品名",
                "text",
                true,
                false,
                true,
                max200,
                None,
                20
            ),
            field(
                "notes",
                "備註",
                "long_text",
                false,
                false,
                false,
                None,
                None,
                30
            ),
        ]
    );
}

/// 銷貨明細的 `product_name` 與 §8.9「明細的品名」一致：插在 `product_id` 之後。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn sales_line_product_name_fields(pool: PgPool) {
    let max200 = || Some(json!({ "max_length": 200 }));
    for (entity, expected) in [
        (
            "sales_order_line",
            field(
                "product_name",
                "品名",
                "text",
                true,
                false,
                false,
                max200(),
                None,
                15,
            ),
        ),
        (
            "shipment_line",
            field(
                "product_name",
                "品名",
                "text",
                true,
                false,
                false,
                max200(),
                None,
                25,
            ),
        ),
        (
            "sales_return_line",
            field(
                "product_name",
                "品名",
                "text",
                false,
                true,
                false,
                None,
                None,
                25,
            ),
        ),
    ] {
        let fields = fields_of(&pool, entity).await;
        let at = fields
            .iter()
            .position(|f| f.field_key == "product_name")
            .unwrap_or_else(|| panic!("{entity} 沒有 product_name"));
        assert_eq!(fields[at], expected, "{entity}");
        assert_eq!(fields[at - 1].field_key, "product_id", "{entity}");
    }
}

// ---------------------------------------------------------------------------
// 資料表
// ---------------------------------------------------------------------------

/// 實體欄位、索引、trigger。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn table_indexes_and_trigger(pool: PgPool) {
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT column_name::text, is_nullable::text FROM information_schema.columns
         WHERE table_schema = current_schema() AND table_name = 'customer_product_names'
         ORDER BY ordinal_position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<&str> = columns.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(
        names,
        [
            "id",
            "customer_id",
            "sort_order",
            "product_id",
            "name",
            "notes",
            "custom_data",
            "formula_errors",
            "created_at",
            "created_by",
            "updated_at",
            "updated_by",
            "deleted_at",
            "version",
        ]
    );

    for table in ["sales_order_lines", "shipment_lines", "sales_return_lines"] {
        let nullable: String = sqlx::query_scalar(
            "SELECT is_nullable::text FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = $1
               AND column_name = 'product_name'",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(nullable, "NO", "{table}.product_name");
    }

    let indexes: Vec<(String, String)> = sqlx::query_as(
        "SELECT indexname::text, indexdef FROM pg_indexes
         WHERE schemaname = current_schema() AND tablename = 'customer_product_names'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let def = |name: &str| {
        indexes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.clone())
            .unwrap_or_else(|| panic!("缺少索引 {name}"))
    };
    assert!(
        def("idx_customer_product_names_customer_id")
            .contains("(customer_id, sort_order, id) WHERE (deleted_at IS NULL)")
    );
    assert!(def("idx_customer_product_names_product_id").contains("(product_id)"));
    assert!(def("idx_customer_product_names_name").contains("lower(name)"));
    assert!(def("idx_customer_product_names_custom_data").contains("gin (custom_data)"));
    let unique = def("uq_customer_product_names_customer_product");
    assert!(unique.starts_with("CREATE UNIQUE INDEX"), "{unique}");
    assert!(
        unique.contains("(customer_id, product_id) WHERE (deleted_at IS NULL)"),
        "{unique}"
    );

    let trigger: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger
         WHERE tgname = 'trg_customer_product_names_set_updated_at'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(trigger, 1);
}

/// 同一客戶同一商品只能有一筆未刪除的客戶品名；刪除後可以再新增，其他客戶不受影響。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn one_name_per_customer_and_product(pool: PgPool) {
    let ([c0, c1], [p0, p1]) = seed(&pool).await;
    let first = insert_name(&pool, c0, p0, "X 型扳手").await.unwrap();

    let err = insert_name(&pool, c0, p0, "Y 型扳手").await.unwrap_err();
    assert_eq!(
        sqlstate(&err),
        (
            "23505".to_owned(),
            Some("uq_customer_product_names_customer_product".to_owned())
        )
    );

    // 同一客戶的其他商品、其他客戶的同一商品都可以
    insert_name(&pool, c0, p1, "鉗子").await.unwrap();
    insert_name(&pool, c1, p0, "工具組零件 03").await.unwrap();

    // 刪除後可以再新增
    sqlx::query("UPDATE customer_product_names SET deleted_at = now() WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    insert_name(&pool, c0, p0, "Y 型扳手").await.unwrap();
}

/// 外鍵與必填。
#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn foreign_keys_and_not_null(pool: PgPool) {
    let ([c0, _], [p0, _]) = seed(&pool).await;

    let err = insert_name(&pool, c0, Uuid::now_v7(), "X")
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err).0, "23503");
    let err = insert_name(&pool, Uuid::now_v7(), p0, "X")
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err).0, "23503");

    let err = sqlx::query(
        "INSERT INTO customer_product_names (id, customer_id, product_id) VALUES ($1, $2, $3)",
    )
    .bind(Uuid::now_v7())
    .bind(c0)
    .bind(p0)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(sqlstate(&err).0, "23502");
}
