//! 銷貨用商品選擇 API 的整合測試（#266，docs/architecture.md §8.9「銷貨用商品選擇 API」）：
//! 以客戶品名搜尋、排序、範圍（停用、已刪除、其他客戶）、參數錯誤與權限。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::{Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

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

async fn user(pool: &PgPool, name: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(name, role))
        .await
        .unwrap();
    token.into_string()
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

const URI: &str = "/api/v1/sales/product-lookup";

async fn lookup(pool: &PgPool, token: &str, query: &str) -> (StatusCode, Value) {
    call(pool, Method::GET, &format!("{URI}?{query}"), token, None).await
}

/// `(code, customer_product_name, product_name)` 依回應順序。
fn rows(body: &Value) -> Vec<(String, Option<String>, String)> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["code"].as_str().unwrap().to_owned(),
                r["customer_product_name"].as_str().map(str::to_owned),
                r["product_name"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn row(code: &str, customer_name: Option<&str>, name: &str) -> (String, Option<String>, String) {
    (
        code.to_owned(),
        customer_name.map(str::to_owned),
        name.to_owned(),
    )
}

struct Master {
    a: Id,
    b: Id,
}

/// 商品 P001 扳手 10mm、P002 鉗子、P003 扳手組、P004 停用的扳手、P005 已刪除的扳手（依建立順序）；
/// 客戶 A：P001 → X 型扳手、P002 → 工具組零件 03、P004 → 停用品名、P005 → 刪除品名；客戶 B 沒有設定。
async fn master_data(pool: &PgPool, token: &str) -> Master {
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let mut ids = Vec::new();
    for (code, name, spec, price) in [
        ("P001", "扳手 10mm", Some("鍍鉻"), Some("120")),
        ("P002", "鉗子", None, None),
        ("P003", "扳手組", None, None),
        ("P004", "停用的扳手", None, None),
        ("P005", "已刪除的扳手", None, None),
    ] {
        let mut data = json!({ "code": code, "name": name, "unit_id": unit });
        if let Some(spec) = spec {
            data["spec"] = json!(spec);
        }
        if let Some(price) = price {
            data["default_sale_price"] = json!(price);
        }
        ids.push(
            create(pool, token, "product", json!({ "data": data }))
                .await
                .id,
        );
    }
    let a = create(
        pool,
        token,
        "customer",
        json!({
            "data": { "code": "C001", "name": "客戶 A" },
            "lines": { "customer_product_name": [
                { "data": { "product_id": ids[0], "name": "X 型扳手" } },
                { "data": { "product_id": ids[1], "name": "工具組零件 03" } },
                { "data": { "product_id": ids[3], "name": "停用品名" } },
                { "data": { "product_id": ids[4], "name": "刪除品名" } },
            ] },
        }),
    )
    .await
    .id;
    let b = create(
        pool,
        token,
        "customer",
        json!({ "data": { "code": "C002", "name": "客戶 B" } }),
    )
    .await
    .id;
    sqlx::query("UPDATE products SET is_active = false WHERE id = $1")
        .bind(ids[3])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE products SET deleted_at = now() WHERE id = $1")
        .bind(ids[4])
        .execute(pool)
        .await
        .unwrap();
    Master { a, b }
}

/// 以客戶品名的一部分找到商品；客戶品名符合的排在前面；料號、商品名稱仍可找到。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn search_by_customer_product_name(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;

    // 「零件」只在客戶品名中
    let (status, body) = lookup(&pool, &token, &format!("customer_id={}&q=零件", m.a)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        [row("P002", Some("工具組零件 03"), "工具組零件 03")]
    );

    // 「扳手」：客戶品名符合的 P001 在前，其他依由新到舊（P003 在 P001 之後建立）；停用、已刪除的不出現
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=扳手", m.a)).await;
    assert_eq!(
        rows(&body),
        [
            row("P001", Some("X 型扳手"), "X 型扳手"),
            row("P003", None, "扳手組"),
        ]
    );
    let first = &body[0];
    assert_eq!(first["title"], "P001 扳手 10mm");
    assert_eq!(first["name"], "扳手 10mm");
    assert_eq!(first["spec"], "鍍鉻");
    assert_eq!(first["product_type"], "goods");
    assert_eq!(first["default_sale_price"], "120.0000");
    assert_eq!(body[1]["spec"], Value::Null);
    assert_eq!(body[1]["default_sale_price"], Value::Null);

    // 料號、商品名稱（客戶品名不含「鉗」）
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=p002", m.a)).await;
    assert_eq!(
        rows(&body),
        [row("P002", Some("工具組零件 03"), "工具組零件 03")]
    );
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=鉗子", m.a)).await;
    assert_eq!(
        rows(&body),
        [row("P002", Some("工具組零件 03"), "工具組零件 03")]
    );

    // 料號與客戶品名混用
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=P001+X+型", m.a)).await;
    assert_eq!(rows(&body), [row("P001", Some("X 型扳手"), "X 型扳手")]);

    // 沒有關鍵字：有客戶品名的在前（各自由新到舊）
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}", m.a)).await;
    let codes: Vec<String> = rows(&body).into_iter().map(|r| r.0).collect();
    assert_eq!(codes, ["P002", "P001", "P003"]);

    // limit
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&limit=1", m.a)).await;
    assert_eq!(rows(&body).len(), 1);
}

/// 其他客戶的客戶品名不會被比對到；沒帶 customer_id 時與一般商品搜尋相同。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn other_customers_and_no_customer(pool: PgPool) {
    let token = user(&pool, "viewer", role::VIEWER).await;
    let editor = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &editor).await;

    let (status, body) = lookup(&pool, &token, &format!("customer_id={}&q=零件", m.b)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(rows(&body), []);
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=扳手", m.b)).await;
    assert_eq!(
        rows(&body),
        [row("P003", None, "扳手組"), row("P001", None, "扳手 10mm")]
    );

    for query in ["q=扳手", "customer_id=&q=扳手"] {
        let (status, body) = lookup(&pool, &token, query).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            rows(&body),
            [row("P003", None, "扳手組"), row("P001", None, "扳手 10mm")],
            "{query}"
        );
    }
    let (_, body) = lookup(&pool, &token, "q=零件").await;
    assert_eq!(rows(&body), []);
}

/// 參數錯誤、客戶不存在或已刪除、沒有權限。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn errors(pool: PgPool) {
    let token = user(&pool, "viewer", role::VIEWER).await;
    let editor = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &editor).await;

    let (status, body) = lookup(&pool, &token, "customer_id=abc").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["field"], "customer_id");

    let (status, body) = lookup(&pool, &token, &format!("customer_id={}", Id::now_v7())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    sqlx::query("UPDATE customers SET deleted_at = now() WHERE id = $1")
        .bind(m.b)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = lookup(&pool, &token, &format!("customer_id={}", m.b)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = lookup(&pool, &token, "limit=0").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = lookup(&pool, &token, "limit=x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // 沒有 record.read → 403
    sqlx::query(
        "DELETE FROM role_permissions
         WHERE permission = 'record.read'
           AND role_id = (SELECT id FROM roles WHERE code = 'viewer')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = lookup(&pool, &token, &format!("customer_id={}", m.a)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

// ---------------------------------------------------------------------------
// 效能量測（#279，docs/architecture.md §8.9「銷貨用商品選擇 API」的「效能」）
// ---------------------------------------------------------------------------

/// 以 SQL 直接建立 `n` 筆商品（中英文混合的料號、名稱），回傳 id（由舊到新）。
async fn bulk_products(pool: &PgPool, unit: Id, n: usize) -> Vec<Id> {
    const WORDS: [&str; 8] = [
        "扳手",
        "Wrench",
        "螺絲",
        "六角螺帽",
        "Socket",
        "鉗子",
        "ＡＢＣ 套筒",
        "Drill Bit",
    ];
    let ids: Vec<Id> = (0..n).map(|_| Id::now_v7()).collect();
    let codes: Vec<String> = (0..n).map(|i| format!("P{i:05}")).collect();
    let names: Vec<String> = (0..n)
        .map(|i| format!("{} {}mm", WORDS[i % WORDS.len()], i % 37))
        .collect();
    sqlx::query(
        "INSERT INTO products (id, code, name, unit_id)
         SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::text[]) AS t(id, code, name), (SELECT $4::uuid)",
    )
    .bind(&ids)
    .bind(&codes)
    .bind(&names)
    .bind(unit)
    .execute(pool)
    .await
    .unwrap();
    ids
}

/// 1 萬筆商品、這位客戶 500 筆客戶品名時，`q` 為空白、一個詞、兩個詞的回應時間（p50、p95）。
///
/// `cargo test --release -p hammer-server --test sales_product_lookup_api -- --ignored --nocapture lookup_performance`
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
#[ignore = "效能量測，手動執行"]
async fn lookup_performance(pool: PgPool) {
    let token = user(&pool, "viewer", role::VIEWER).await;
    let editor = user(&pool, "editor", role::EDITOR).await;
    let unit = create(
        &pool,
        &editor,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let products = bulk_products(&pool, unit, 10_000).await;
    let customer = create(
        &pool,
        &editor,
        "customer",
        json!({ "data": { "code": "C001", "name": "客戶 A" } }),
    )
    .await
    .id;
    // 每 20 筆商品一筆客戶品名（500 筆）
    let picked: Vec<Id> = products.iter().step_by(20).copied().collect();
    let cpn_ids: Vec<Id> = picked.iter().map(|_| Id::now_v7()).collect();
    let cpn_names: Vec<String> = (0..picked.len())
        .map(|i| format!("客戶專用零件 {i:03}"))
        .collect();
    sqlx::query(
        "INSERT INTO customer_product_names (id, customer_id, sort_order, product_id, name)
         SELECT t.id, $2, t.ord::int, t.product_id, t.name
         FROM UNNEST($1::uuid[], $3::uuid[], $4::text[]) WITH ORDINALITY AS t(id, product_id, name, ord)",
    )
    .bind(&cpn_ids)
    .bind(customer)
    .bind(&picked)
    .bind(&cpn_names)
    .execute(&pool)
    .await
    .unwrap();

    for q in ["", "扳手", "零件", "wrench 12mm", "zzzz", "零件 999"] {
        let uri = format!("customer_id={customer}&q={}", urlencode(q));
        for _ in 0..5 {
            lookup(&pool, &token, &uri).await;
        }
        let mut times = Vec::new();
        let mut count = 0;
        for _ in 0..50 {
            let start = std::time::Instant::now();
            let (status, body) = lookup(&pool, &token, &uri).await;
            times.push(start.elapsed());
            assert_eq!(status, StatusCode::OK, "{body}");
            count = body.as_array().unwrap().len();
        }
        times.sort();
        println!(
            "q={q:?}: {count} 筆, p50 {:?}, p95 {:?}",
            times[times.len() / 2],
            times[times.len() * 95 / 100]
        );
    }
}

/// 百分比編碼（查詢字串用）。
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 回歸測試（#279）：SQL 篩選與逐批產生標題比對（M11 的做法）的結果、排序完全相同
// ---------------------------------------------------------------------------

fn reader() -> hammer_core::auth::AuthContext {
    hammer_core::auth::AuthContext {
        user_id: Id::now_v7(),
        username: "reader".into(),
        display_name: "reader".into(),
        role: role::VIEWER.into(),
        permissions: [hammer_shared::permission::RECORD_READ.to_owned()]
            .into_iter()
            .collect(),
        session_id: Id::now_v7(),
        must_change_password: false,
    }
}

/// 對每位客戶（含不指定）、每個關鍵字、每個筆數上限，比較兩種做法的結果。
async fn assert_same_as_scan(pool: &PgPool, customers: &[Option<Id>], queries: &[&str]) {
    use hammer_core::sales::product_lookup::{product_lookup, product_lookup_by_scan};
    let ctx = reader();
    let mut compared = 0;
    for customer in customers {
        for q in queries {
            for limit in ["1", "3", "20", "50"] {
                let mut pairs = vec![
                    ("q".to_owned(), (*q).to_owned()),
                    ("limit".to_owned(), limit.to_owned()),
                ];
                if let Some(c) = customer {
                    pairs.push(("customer_id".to_owned(), c.to_string()));
                }
                let fast = product_lookup(pool, &ctx, &pairs).await.unwrap();
                let scan = product_lookup_by_scan(pool, &ctx, &pairs).await.unwrap();
                assert_eq!(fast, scan, "customer={customer:?} q={q:?} limit={limit}");
                compared += usize::from(!fast.is_empty());
            }
        }
    }
    // 確認測試資料真的有符合的結果（不是全部空陣列）
    assert!(compared > queries.len(), "{compared}");
}

/// 中英文大小寫、全形、重音字母、`%` `_` `\` 等特殊字元，預設範本與自訂範本都與逐批比對相同。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sql_filter_matches_scan(pool: PgPool) {
    let editor = user(&pool, "editor", role::EDITOR).await;
    let unit = create(
        &pool,
        &editor,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let names = [
        ("P001", "扳手 10mm"),
        ("P0011", "扳手組"),
        ("p002", "WRENCH Large"),
        ("P003", "wrench small"),
        ("P004", "ＡＢＣ 全形套筒"),
        ("P005", "ａｂｃ 小寫全形"),
        ("P006", "Café 咖啡色鉗子"),
        ("P007", "CAFÉ 大寫"),
        ("P008", "折扣 10% 螺絲"),
        ("P009", "snake_case 零件"),
        ("P010", "路徑 C:\\tools"),
        ("P011", "100 螺帽"),
        ("P012", "停用的扳手"),
        ("P013", "已刪除的扳手"),
        ("X-01", "鉗子"),
    ];
    let mut ids = Vec::new();
    for (code, name) in names {
        let data = json!({ "code": code, "name": name, "unit_id": unit });
        ids.push(
            create(&pool, &editor, "product", json!({ "data": data }))
                .await
                .id,
        );
    }
    let cpn = |i: usize, name: &str| json!({ "data": { "product_id": ids[i], "name": name } });
    let a = create(
        &pool,
        &editor,
        "customer",
        json!({
            "data": { "code": "C001", "name": "客戶 A" },
            "lines": { "customer_product_name": [
                cpn(0, "X 型扳手"), cpn(2, "客戶的 wrench"), cpn(5, "ＡＢＣ 客戶全形"),
                cpn(8, "折扣_10%"), cpn(9, "snake 客戶"), cpn(11, "停用品名扳手"),
                cpn(12, "刪除品名扳手"), cpn(14, "工具組零件 03"),
            ] },
        }),
    )
    .await
    .id;
    let b = create(
        &pool,
        &editor,
        "customer",
        json!({
            "data": { "code": "C002", "name": "客戶 B" },
            "lines": { "customer_product_name": [ cpn(1, "B 的扳手"), cpn(6, "café B") ] },
        }),
    )
    .await
    .id;
    sqlx::query("UPDATE products SET is_active = false WHERE id = $1")
        .bind(ids[11])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE products SET deleted_at = now() WHERE id = $1")
        .bind(ids[12])
        .execute(&pool)
        .await
        .unwrap();

    let customers = [Some(a), Some(b), None];
    let queries = [
        "",
        "扳手",
        "P001",
        "p00",
        "wrench",
        "WRENCH",
        "Wrench l",
        "ａｂｃ",
        "ＡＢＣ",
        "abc",
        "café",
        "CAFÉ",
        "%",
        "10%",
        "_",
        "snake_case",
        "e_c",
        "\\",
        "c:\\",
        "零件",
        "x 型",
        "p001 x",
        "客戶",
        "100",
        "x-01 鉗",
        "不存在",
    ];
    assert_same_as_scan(&pool, &customers, &queries).await;

    // 自訂範本：第二組逐批比對，第一組仍由 SQL 取得
    sqlx::query(
        "UPDATE entity_definitions SET title_template = '{name}（{code}）' WHERE key = 'product'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_same_as_scan(&pool, &customers, &queries).await;
    let token = user(&pool, "viewer", role::VIEWER).await;
    let (_, body) = lookup(&pool, &token, &format!("customer_id={a}&q=扳手")).await;
    assert_eq!(body[0]["title"], "扳手 10mm（P001）");
}

/// 關鍵字中的 `%`、`_`、`\` 視為一般字元。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn wildcards_are_literal(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let unit = create(
        &pool,
        &token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    for (code, name) in [
        ("P1", "折扣 10% 螺絲"),
        ("P2", "一般 105 螺絲"),
        ("P3", "a_b"),
        ("P4", "axb"),
    ] {
        let data = json!({ "code": code, "name": name, "unit_id": unit });
        create(&pool, &token, "product", json!({ "data": data })).await;
    }
    let codes = |body: &Value| -> Vec<String> {
        body.as_array()
            .unwrap()
            .iter()
            .map(|r| r["code"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, body) = lookup(&pool, &token, "q=10%25").await;
    assert_eq!(codes(&body), ["P1"]);
    let (_, body) = lookup(&pool, &token, "q=a_b").await;
    assert_eq!(codes(&body), ["P3"]);
    let (_, body) = lookup(&pool, &token, "q=%25").await;
    assert_eq!(codes(&body), ["P1"]);
}
