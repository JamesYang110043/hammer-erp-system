//! 客戶品名批次寫入 API 的整合測試（#283，docs/architecture.md §8.9「客戶品名批次寫入 API」）：
//! dry_run、upsert、全有或全無、指定方式、重複、客戶的 version 與操作紀錄、權限。
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

async fn user(pool: &PgPool) -> String {
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

fn lines<'a>(record: &'a Record, detail: &str) -> &'a [Record] {
    &record.lines.as_ref().unwrap()[detail]
}

const URI: &str = "/api/v1/sales/customer-product-names/batch";

async fn batch(pool: &PgPool, token: &str, dry_run: bool, rows: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        URI,
        token,
        Some(json!({ "dry_run": dry_run, "rows": rows })),
    )
    .await
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(entity, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 客戶的客戶品名 `(料號標題, 名稱, 備註)` 依順序。
fn cpn(customer: &Record) -> Vec<(String, String, Value)> {
    lines(customer, "customer_product_name")
        .iter()
        .map(|l| {
            (
                l.relations["product_id"].title.clone(),
                l.data["name"].as_str().unwrap().to_owned(),
                l.data.get("notes").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

fn t(product: &str, name: &str, notes: Value) -> (String, String, Value) {
    (product.to_owned(), name.to_owned(), notes)
}

/// `(action, [(field, message)])` 依順序。
fn results(body: &Value) -> Vec<(Value, Vec<(Value, String)>)> {
    body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["action"].clone(),
                r["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| {
                        (
                            e["field"].clone(),
                            e["message"].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

async fn audit_count(pool: &PgPool, customer: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE entity_key = 'customer' AND record_id = $1",
    )
    .bind(customer)
    .fetch_one(pool)
    .await
    .unwrap()
}

struct Master {
    /// C001（P001 → X 型扳手，備註「舊名稱」）、P003 → 零件 03。
    a: Id,
    /// C002 沒有客戶品名。
    b: Id,
    p1: Id,
    p2: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let mut p = Vec::new();
    for (code, name) in [
        ("P001", "扳手 10mm"),
        ("P002", "鉗子"),
        ("P003", "零件"),
        ("P004", "停用商品"),
    ] {
        p.push(
            create(
                pool,
                token,
                "product",
                json!({ "data": { "code": code, "name": name, "unit_id": unit } }),
            )
            .await
            .id,
        );
    }
    sqlx::query("UPDATE products SET is_active = false WHERE id = $1")
        .bind(p[3])
        .execute(pool)
        .await
        .unwrap();
    let a = create(
        pool,
        token,
        "customer",
        json!({
            "data": { "code": "C001", "name": "客戶 A" },
            "lines": { "customer_product_name": [
                { "data": { "product_id": p[0], "name": "X 型扳手", "notes": "舊名稱" } },
                { "data": { "product_id": p[2], "name": "零件 03" } },
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
    Master {
        a,
        b,
        p1: p[0],
        p2: p[1],
    }
}

/// dry_run 只回傳動作，不寫入；寫入時新增排在最後、修改保留沒帶的備註，客戶 version + 1、一筆操作紀錄。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn dry_run_then_write(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;
    let before = get(&pool, &token, "customer", m.a).await;
    let audits = audit_count(&pool, m.a).await;

    let rows = json!([
        { "customer_code": "C001", "product_code": "P001", "name": "X 型扳手" },
        { "customer_code": "c001", "product_code": "p003", "name": " 零件 03（新） " },
        { "customer_code": "C001", "product_code": "P002", "name": "尖嘴鉗", "notes": " 開單時設定 " },
        { "customer_id": m.b, "product_id": m.p1, "name": "B 的扳手" },
        { "customer_code": "C001", "product_code": "P004", "name": "停用商品也可以" },
    ]);
    let (status, body) = batch(&pool, &token, true, rows.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dry_run"], true);
    assert_eq!(
        body["summary"],
        json!({ "create": 3, "update": 1, "unchanged": 1, "error": 0 })
    );
    let actions: Vec<Value> = results(&body).into_iter().map(|r| r.0).collect();
    assert_eq!(
        actions,
        [
            json!("unchanged"),
            json!("update"),
            json!("create"),
            json!("create"),
            json!("create")
        ]
    );
    assert_eq!(body["rows"][3]["customer_id"], json!(m.b));
    assert_eq!(body["rows"][3]["product_id"], json!(m.p1));
    // 沒有寫入
    let after = get(&pool, &token, "customer", m.a).await;
    assert_eq!(after.version, before.version);
    assert_eq!(audit_count(&pool, m.a).await, audits);

    let (status, body) = batch(&pool, &token, false, rows).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dry_run"], false);
    let a = get(&pool, &token, "customer", m.a).await;
    assert_eq!(a.version, before.version + 1);
    assert_eq!(
        cpn(&a),
        [
            t("P001 扳手 10mm", "X 型扳手", json!("舊名稱")),
            t("P003 零件", "零件 03（新）", Value::Null),
            t("P002 鉗子", "尖嘴鉗", json!("開單時設定")),
            t("P004 停用商品", "停用商品也可以", Value::Null),
        ]
    );
    let b = get(&pool, &token, "customer", m.b).await;
    assert_eq!(cpn(&b), [t("P001 扳手 10mm", "B 的扳手", Value::Null)]);
    assert_eq!(audit_count(&pool, m.a).await, audits + 1);

    // 操作紀錄的格式與客戶 API 相同：lines.customer_product_name 的修改前後
    let (before_data, after_data): (Value, Value) = sqlx::query_as(
        "SELECT before_data, after_data FROM audit_logs
         WHERE entity_key = 'customer' AND record_id = $1 ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(m.a)
    .fetch_one(&pool)
    .await
    .unwrap();
    let names = |v: &Value| -> Vec<String> {
        v["lines.customer_product_name"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                assert!(l["id"].is_string());
                l["data"]["name"].as_str().unwrap().to_owned()
            })
            .collect()
    };
    assert_eq!(names(&before_data), ["X 型扳手", "零件 03"]);
    assert_eq!(
        names(&after_data),
        ["X 型扳手", "零件 03（新）", "尖嘴鉗", "停用商品也可以"]
    );
    assert_eq!(before_data.as_object().unwrap().len(), 1);

    // 以舊的 version 存檔客戶 → 409
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("customer", m.a),
        &token,
        Some(json!({ "version": before.version, "data": { "name": "改名" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // 帶 notes: null → 清空；全部不變 → 不寫入、version 不變
    let (status, body) = batch(
        &pool,
        &token,
        false,
        json!([{ "customer_code": "C001", "product_code": "P001", "name": "X 型扳手", "notes": null }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let a2 = get(&pool, &token, "customer", m.a).await;
    assert_eq!(cpn(&a2)[0], t("P001 扳手 10mm", "X 型扳手", Value::Null));
    let (status, body) = batch(
        &pool,
        &token,
        false,
        json!([{ "customer_code": "C001", "product_code": "P001", "name": "X 型扳手" }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["summary"]["unchanged"], 1);
    assert_eq!(
        get(&pool, &token, "customer", m.a).await.version,
        a2.version
    );
}

/// 每列的錯誤；寫入時任一列有錯誤整批不寫入（422，details 與 dry_run 回應同格式）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn errors_reject_whole_batch(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;
    let deleted = create(
        &pool,
        &token,
        "customer",
        json!({ "data": { "code": "C009", "name": "已刪除" } }),
    )
    .await
    .id;
    sqlx::query("UPDATE customers SET deleted_at = now() WHERE id = $1")
        .bind(deleted)
        .execute(&pool)
        .await
        .unwrap();
    let rows = json!([
        { "customer_code": "C001", "product_code": "P002", "name": "尖嘴鉗" },
        { "customer_code": "C404", "product_code": "P404", "name": "X" },
        { "customer_code": "C009", "product_code": "P001", "name": "X" },
        { "customer_id": m.a, "product_id": Id::now_v7(), "name": "X" },
        { "customer_code": "C001", "product_id": m.p1, "name": "X" },
        { "name": "X" },
        { "customer_code": " ", "product_code": "P001", "name": "X" },
        { "customer_code": "C001", "product_code": "P001", "name": " " },
        { "customer_code": "C001", "product_code": "P001", "name": "字".repeat(201) },
        { "customer_code": "C001", "product_code": "P001", "name": "a\u{7}b" },
        { "customer_id": m.a, "product_id": m.p2, "name": "重複" },
    ]);
    let (status, body) = batch(&pool, &token, true, rows.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let s = |v: &str| json!(v);
    assert_eq!(
        results(&body),
        [
            (json!("create"), vec![]),
            (
                Value::Null,
                vec![
                    (s("customer_code"), "找不到客戶編號 C404".into()),
                    (s("product_code"), "找不到料號 P404".into())
                ]
            ),
            (
                Value::Null,
                vec![(s("customer_code"), "找不到客戶編號 C009".into())]
            ),
            (Value::Null, vec![(s("product_id"), "找不到此商品".into())]),
            (
                Value::Null,
                vec![(Value::Null, "請以編號或 id 指定客戶與商品（二擇一）".into())]
            ),
            (
                Value::Null,
                vec![(Value::Null, "請以編號或 id 指定客戶與商品（二擇一）".into())]
            ),
            (
                Value::Null,
                vec![(s("customer_code"), "客戶編號不可空白".into())]
            ),
            (Value::Null, vec![(s("name"), "此欄位為必填".into())]),
            (
                Value::Null,
                vec![
                    (s("name"), "不可超過 200 個字元".into()),
                    // 同一客戶、同一商品，即使品名有錯也標示重複
                    (Value::Null, "與第 8 列重複（同一客戶、同一商品）".into()),
                ]
            ),
            (
                Value::Null,
                vec![
                    (s("name"), "客戶品名不可包含控制字元".into()),
                    (Value::Null, "與第 8 列重複（同一客戶、同一商品）".into()),
                ]
            ),
            (
                Value::Null,
                vec![(Value::Null, "與第 1 列重複（同一客戶、同一商品）".into())]
            ),
        ]
    );
    assert_eq!(body["rows"][10]["errors"][0]["duplicate_of"], 0);
    assert_eq!(
        body["summary"],
        json!({ "create": 1, "update": 0, "unchanged": 0, "error": 10 })
    );

    let before = get(&pool, &token, "customer", m.a).await;
    let (status, body) = batch(&pool, &token, false, rows).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "VALIDATION");
    assert_eq!(body["message"], "有 10 列錯誤，未寫入");
    assert_eq!(body["details"]["dry_run"], false);
    assert_eq!(body["details"]["summary"]["error"], 10);
    assert_eq!(body["details"]["rows"].as_array().unwrap().len(), 11);
    let after = get(&pool, &token, "customer", m.a).await;
    assert_eq!(after.version, before.version);
    assert_eq!(cpn(&after).len(), 2);
    let _ = m.b;
}

/// rows 的列數、request 格式、權限。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn rows_limit_and_permission(pool: PgPool) {
    let token = user(&pool).await;
    master_data(&pool, &token).await;
    let (status, body) = batch(&pool, &token, true, json!([])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["field"], "rows");
    let row = json!({ "customer_code": "C001", "product_code": "P001", "name": "X" });
    let many = Value::Array(vec![row.clone(); 5001]);
    let (status, body) = batch(&pool, &token, true, many).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["field"], "rows");
    let (status, body) = batch(&pool, &token, true, Value::Array(vec![row.clone(); 5000])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["summary"]["error"], 4999);

    let (status, _) = call(
        &pool,
        Method::POST,
        URI,
        &token,
        Some(json!({ "rows": [] })),
    )
    .await;
    assert!(status.is_client_error(), "{status}");

    let (_, viewer) = create_user_with_session(&pool, &TestUser::new("viewer", role::VIEWER))
        .await
        .unwrap();
    let (status, body) = batch(&pool, viewer.as_str(), true, json!([row])).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}
