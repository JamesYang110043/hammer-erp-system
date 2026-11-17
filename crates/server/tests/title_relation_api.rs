//! 標題範本引用關聯欄位的 API 整合測試（#111，docs/architecture.md §8.5）。
//!
//! - `{record_no} {customer.short_name}` 在列表、單筆、`lookup`、`relations` 中產生正確標題
//! - 系統資料類別的範本也可以引用關聯（商品的 `{code} {name}（{unit_id.name}）`）
//! - 不合法的關聯引用 → `422`（`details.field = title_template`）
//! - 列表的資料庫查詢次數不隨筆數增加（計算 `sqlx::query` 的 tracing 事件）
//!
//! 各型態的格式、停用欄位、找不到目標資料等細節在 `hammer_core::title` 的單元測試。
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::pagination::Page;
use hammer_shared::record::Record;
use hammer_shared::schema::EntityDetail;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

/// 有關聯欄位的自訂資料類別（維修單）。
const REPAIR: &str = "repair_order";
/// 關聯到維修單的自訂資料類別（派工），用來檢查 `relations` 的標題。
const VISIT: &str = "visit";

fn app(pool: &PgPool) -> Router {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    router(AppState::new(pool.clone(), config))
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
    let res = app(pool).oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

tokio::task_local! {
    /// 目前請求執行的 SQL 陳述式數量；只在 [`call_counting`] 內設定。
    static QUERY_COUNT: Arc<AtomicUsize>;
}

/// 計算 `sqlx::query` 的 tracing 事件（每執行一個 SQL 陳述式一次），計入目前 task 的 [`QUERY_COUNT`]。
///
/// 設為全域 subscriber（而不是每次量測各自設定），平行執行的測試才不會互相影響 tracing 的
/// callsite 快取。
struct QueryCounter;

impl<S: tracing::Subscriber> Layer<S> for QueryCounter {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            let _ = QUERY_COUNT.try_with(|c| c.fetch_add(1, Ordering::SeqCst));
        }
    }
}

/// 同 [`call`]（GET），另外回傳處理請求時執行的 SQL 陳述式數量。
async fn call_counting(pool: &PgPool, uri: &str, token: &str) -> (StatusCode, Value, usize) {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        tracing::subscriber::set_global_default(Registry::default().with(QueryCounter)).unwrap();
    });
    let count = Arc::new(AtomicUsize::new(0));
    let (status, body) = QUERY_COUNT
        .scope(count.clone(), call(pool, Method::GET, uri, token, None))
        .await;
    (status, body, count.load(Ordering::SeqCst))
}

/// 422 VALIDATION，且 `details.field` 為 `title_template`。
fn assert_invalid_template(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation, "{body}");
    assert!(!err.message.is_empty());
    assert_eq!(err.details.unwrap()["field"], "title_template", "{body}");
}

async fn admin_token(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    token.into_string()
}

async fn entity_id(pool: &PgPool, key: &str) -> Id {
    sqlx::query_scalar("SELECT id FROM entity_definitions WHERE key = $1")
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn create_entity(pool: &PgPool, token: &str, body: Value) {
    let (status, body) = call(pool, Method::POST, "/api/v1/entities", token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn create_field(pool: &PgPool, token: &str, entity: &str, body: Value) {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

fn relation_field(key: &str, target: Id) -> Value {
    json!({ "field_key": key, "label": key, "data_type": "relation", "relation_entity_id": target })
}

async fn set_template(
    pool: &PgPool,
    token: &str,
    entity: &str,
    template: &str,
) -> (StatusCode, Value) {
    let uri = format!("/api/v1/entities/{entity}");
    let (status, body) = call(pool, Method::GET, &uri, token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = serde_json::from_value::<EntityDetail>(body).unwrap().entity;
    call(
        pool,
        Method::PATCH,
        &uri,
        token,
        Some(json!({
            "version": e.version, "label": e.label, "icon": e.icon,
            "numbering_pattern": e.numbering_pattern, "title_template": template,
            "sort_order": e.sort_order, "is_active": e.is_active,
        })),
    )
    .await
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

async fn create(pool: &PgPool, token: &str, entity: &str, data: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &records_uri(entity),
        token,
        Some(json!({ "data": data })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let uri = format!("{}/{id}", records_uri(entity));
    let (status, body) = call(pool, Method::GET, &uri, token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn list_titles(pool: &PgPool, token: &str, entity: &str) -> Vec<String> {
    let (status, body) = call(pool, Method::GET, &records_uri(entity), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    page.items.into_iter().map(|r| r.title).collect()
}

async fn lookup_titles(pool: &PgPool, token: &str, entity: &str, q: &str) -> Vec<String> {
    let uri = format!("{}/lookup?q={q}", records_uri(entity));
    let (status, body) = call(pool, Method::GET, &uri, token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_owned())
        .collect()
}

/// 建立 [`REPAIR`]（單號 `RO-{000}`；`note` text、`customer` relation → `customer`）與
/// [`VISIT`]（`repair` relation → [`REPAIR`]）。
async fn setup(pool: &PgPool, token: &str) {
    create_entity(
        pool,
        token,
        json!({ "key": REPAIR, "label": "維修單", "kind": "document", "numbering_pattern": "RO-{000}" }),
    )
    .await;
    create_field(
        pool,
        token,
        REPAIR,
        json!({ "field_key": "note", "label": "備註", "data_type": "text" }),
    )
    .await;
    let customer = entity_id(pool, "customer").await;
    create_field(pool, token, REPAIR, relation_field("customer", customer)).await;

    create_entity(
        pool,
        token,
        json!({ "key": VISIT, "label": "派工", "kind": "master" }),
    )
    .await;
    let repair = entity_id(pool, REPAIR).await;
    create_field(pool, token, VISIT, relation_field("repair", repair)).await;
}

async fn customer(pool: &PgPool, token: &str, code: &str, short_name: &str) -> Record {
    create(
        pool,
        token,
        "customer",
        json!({ "code": code, "name": format!("{short_name}股份有限公司"), "short_name": short_name }),
    )
    .await
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_relation_references_are_rejected(pool: PgPool) {
    let token = admin_token(&pool).await;
    setup(&pool, &token).await;

    for template in [
        // 前半段不是本資料類別的欄位
        "{record_no} {vendor.short_name}",
        // 前半段不是關聯欄位
        "{note.short_name}",
        "{record_no.short_name}",
        // 後半段不是目標資料類別（客戶）的欄位；本資料類別的欄位也不算
        "{customer.missing}",
        "{customer.note}",
        // 兩層
        "{customer.short_name.x}",
        // 語法
        "{customer.}",
        "{customer.Short_name}",
    ] {
        let (status, body) = set_template(&pool, &token, REPAIR, template).await;
        assert_invalid_template(status, &body);
    }
    // 系統資料類別：商品的 unit_id 指向單位
    for template in ["{unit_id.missing}", "{code.name}"] {
        let (status, body) = set_template(&pool, &token, "product", template).await;
        assert_invalid_template(status, &body);
    }

    // 合法：目標的系統欄位、單號、目標的關聯欄位（標題中為空字串）
    for template in [
        "{record_no} {customer.short_name}",
        "{customer.record_no}",
        "{customer.code} {customer.credit_limit}",
    ] {
        let (status, body) = set_template(&pool, &token, REPAIR, template).await;
        assert_eq!(status, StatusCode::OK, "{template} {body}");
    }
    let (status, body) =
        set_template(&pool, &token, "product", "{code} {name}（{unit_id.name}）").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn relation_references_in_list_get_lookup_and_relations(pool: PgPool) {
    let token = admin_token(&pool).await;
    setup(&pool, &token).await;
    let (status, body) =
        set_template(&pool, &token, REPAIR, "{record_no} {customer.short_name}").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let tatung = customer(&pool, &token, "C001", "大同").await;
    let sampo = customer(&pool, &token, "C002", "聲寶").await;
    let r1 = create(&pool, &token, REPAIR, json!({ "customer": tatung.id })).await;
    let r2 = create(&pool, &token, REPAIR, json!({ "customer": sampo.id })).await;
    let r3 = create(&pool, &token, REPAIR, json!({ "note": "沒有客戶" })).await;

    // 寫入的回應、單筆
    assert_eq!(r1.title, "RO-001 大同");
    assert_eq!(get(&pool, &token, REPAIR, r2.id).await.title, "RO-002 聲寶");
    // 關聯欄位沒有值 → 空字串（去除前後空白）
    assert_eq!(r3.title, "RO-003");
    // 列表
    assert_eq!(
        list_titles(&pool, &token, REPAIR).await,
        ["RO-003", "RO-002 聲寶", "RO-001 大同"]
    );
    // 選擇器依標題比對（含關聯引用的文字）
    assert_eq!(
        lookup_titles(&pool, &token, REPAIR, "%E8%81%B2%E5%AF%B6").await,
        ["RO-002 聲寶"]
    );
    // 其他資料的 relations：目標範本中的關聯引用也讀取
    let visit = create(&pool, &token, VISIT, json!({ "repair": r1.id })).await;
    assert_eq!(visit.relations["repair"].title, "RO-001 大同");
    assert_eq!(
        get(&pool, &token, VISIT, visit.id).await.relations["repair"].title,
        "RO-001 大同"
    );

    // 讀取目標目前的值
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("{}/{}", records_uri("customer"), tatung.id),
        &token,
        Some(json!({ "version": tatung.version, "data": { "short_name": "大同電器" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tatung: Record = serde_json::from_value(body).unwrap();
    assert_eq!(
        get(&pool, &token, REPAIR, r1.id).await.title,
        "RO-001 大同電器"
    );

    // 目標已軟刪除時仍讀取其資料
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!(
            "{}/{}?version={}",
            records_uri("customer"),
            tatung.id,
            tatung.version
        ),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        get(&pool, &token, REPAIR, r1.id).await.title,
        "RO-001 大同電器"
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_entity_template_references_relation(pool: PgPool) {
    let token = admin_token(&pool).await;
    let (status, body) =
        set_template(&pool, &token, "product", "{code} {name}（{unit_id.name}）").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let unit = create(
        &pool,
        &token,
        "unit",
        json!({ "code": "pcs", "name": "個" }),
    )
    .await;
    let product = create(
        &pool,
        &token,
        "product",
        json!({ "code": "P001", "name": "壓縮機", "unit_id": unit.id }),
    )
    .await;
    assert_eq!(product.title, "P001 壓縮機（個）");
    assert_eq!(
        get(&pool, &token, "product", product.id).await.title,
        "P001 壓縮機（個）"
    );
    assert_eq!(
        list_titles(&pool, &token, "product").await,
        ["P001 壓縮機（個）"]
    );
    assert_eq!(
        lookup_titles(&pool, &token, "product", "P001").await,
        ["P001 壓縮機（個）"]
    );

    // 自訂資料類別指向商品：relations 的標題也套用商品範本的關聯引用
    create_entity(
        &pool,
        &token,
        json!({ "key": "quote", "label": "報價", "kind": "master" }),
    )
    .await;
    let product_entity = entity_id(&pool, "product").await;
    create_field(
        &pool,
        &token,
        "quote",
        relation_field("product", product_entity),
    )
    .await;
    let quote = create(&pool, &token, "quote", json!({ "product": product.id })).await;
    assert_eq!(quote.relations["product"].title, "P001 壓縮機（個）");
}

/// 建立維修單（客戶依序輪流）與指向它的派工。
async fn add_repairs(
    pool: &PgPool,
    token: &str,
    customers: &[Record],
    range: std::ops::Range<usize>,
) {
    for i in range {
        let c = &customers[i % customers.len()];
        let r = create(pool, token, REPAIR, json!({ "customer": c.id })).await;
        create(pool, token, VISIT, json!({ "repair": r.id })).await;
    }
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_query_count_does_not_grow_with_rows(pool: PgPool) {
    let token = admin_token(&pool).await;
    setup(&pool, &token).await;
    let (status, body) =
        set_template(&pool, &token, REPAIR, "{record_no} {customer.short_name}").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut customers = Vec::new();
    for i in 0..5 {
        customers.push(customer(&pool, &token, &format!("C{i:03}"), &format!("客戶{i}")).await);
    }
    add_repairs(&pool, &token, &customers, 0..5).await;

    let measure = async |entity: &str| {
        let uri = format!("{}?page_size=50", records_uri(entity));
        let (status, body, queries) = call_counting(&pool, &uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let page: Page<Record> = serde_json::from_value(body).unwrap();
        (page.items, queries)
    };

    let (items, repair_5) = measure(REPAIR).await;
    assert_eq!(items.len(), 5);
    let (visits, visit_5) = measure(VISIT).await;
    assert_eq!(visits.len(), 5);
    // 確認 tracing 事件有被計算（沒有計算到時兩者都是 0，比較沒有意義）
    assert!(repair_5 > 0 && visit_5 > 0);

    add_repairs(&pool, &token, &customers, 5..50).await;
    let (items, repair_50) = measure(REPAIR).await;
    assert_eq!(items.len(), 50);
    assert!(
        items
            .iter()
            .all(|r| r.title.starts_with("RO-") && r.title.contains("客戶"))
    );
    let (visits, visit_50) = measure(VISIT).await;
    assert_eq!(visits.len(), 50);
    assert!(
        visits
            .iter()
            .all(|v| v.relations["repair"].title.contains("客戶"))
    );

    assert_eq!(repair_50, repair_5, "維修單列表的查詢次數隨筆數增加");
    assert_eq!(visit_50, visit_5, "派工列表的查詢次數隨筆數增加");
}
