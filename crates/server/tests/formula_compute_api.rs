//! 存檔時計算公式的 API 整合測試（#91）。
//!
//! - 新增資料時計算本筆欄位、關聯引用、子表彙總的公式；明細的公式先算，主表的彙總使用明細的結果
//! - 修改明細後主表的彙總更新；修改時所有公式都重算
//! - 計算失敗（除以零）時存檔成功，欄位沒有值，`formula_errors` 有原因
//! - request 帶公式欄位的值時忽略
//! - 依公式欄位排序、篩選
//! - 關聯目標修改後，引用它的資料不變；該筆資料再次存檔後更新（目標已刪除時仍讀取）
//! - 操作紀錄的 diff 包含公式欄位的值
//!
//! 計算的細節（順序、空值、錯誤）在 `hammer_core::formulas` 與 `hammer_formula` 的單元測試。
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::pagination::Page;
use hammer_shared::record::Record;
use hammer_shared::schema::EntityDetail;
use hammer_shared::time::today_taipei;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// 關聯目標（客戶）。
const CUSTOMER: &str = "customer";
/// 有公式欄位的主表（維修單）。
const REPAIR: &str = "repair_order";
/// 維修單的明細子表（零件）。
const PARTS: &str = "parts";

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

async fn create_entity(pool: &PgPool, admin: &str, body: Value) -> EntityDetail {
    let (status, body) = call(pool, Method::POST, "/api/v1/entities", admin, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_field(pool: &PgPool, admin: &str, entity: &str, field: Value) {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        admin,
        Some(field),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// 公式欄位的 request。`decimal` / `money` 結果時小數位數 2。
fn formula_field(key: &str, formula: &str, result: &str) -> Value {
    let mut body = json!({
        "field_key": key,
        "label": format!("{key} 公式"),
        "data_type": "formula",
        "formula": formula,
        "formula_result_type": result,
    });
    if matches!(result, "decimal" | "money") {
        body["decimal_places"] = json!(2);
    }
    body
}

/// 建立以下資料類別，回傳管理者 token。
///
/// | 資料類別 | 欄位 | 公式 |
/// |---|---|---|
/// | [`CUSTOMER`] | `name`（text）、`discount`（decimal） | |
/// | [`PARTS`] | `qty`（integer）、`unit`（money） | `amount = {qty} * {unit}`、`per = {unit} / {qty}` |
/// | [`REPAIR`] | `qty`（integer）、`price`（money）、`divisor`（integer）、`note`（text）、`customer`（relation） | 見下方 |
///
/// [`REPAIR`] 的公式（故意先定義引用其他公式的欄位，計算順序依相依圖）：
///
/// - `grand_total = {subtotal} + {parts_total}`
/// - `subtotal = {qty} * {price}`
/// - `discounted = {subtotal} * (1 - COALESCE({customer.discount}, 0))`
/// - `parts_total = SUM(parts.amount)`（明細的公式）、`part_count = COUNT(parts.amount)`
/// - `ratio = {qty} / {divisor}`
/// - `label = {note} & "-" & {qty}`（文字）
/// - `today = TODAY()`（日期）
async fn setup(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    let admin = token.into_string();

    let customer = create_entity(
        pool,
        &admin,
        json!({ "key": CUSTOMER, "label": "客戶", "kind": "master" }),
    )
    .await;
    for f in [
        json!({ "field_key": "name", "label": "名稱", "data_type": "text" }),
        json!({ "field_key": "discount", "label": "折扣率", "data_type": "decimal", "decimal_places": 2 }),
    ] {
        create_field(pool, &admin, CUSTOMER, f).await;
    }

    let repair = create_entity(
        pool,
        &admin,
        json!({ "key": REPAIR, "label": "維修單", "kind": "document" }),
    )
    .await;
    for f in [
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer" }),
        json!({ "field_key": "price", "label": "單價", "data_type": "money", "decimal_places": 2 }),
        json!({ "field_key": "divisor", "label": "除數", "data_type": "integer" }),
        json!({ "field_key": "note", "label": "備註", "data_type": "text" }),
        json!({ "field_key": "customer", "label": "客戶", "data_type": "relation",
                "relation_entity_id": customer.entity.id }),
    ] {
        create_field(pool, &admin, REPAIR, f).await;
    }

    create_entity(
        pool,
        &admin,
        json!({ "key": PARTS, "label": "零件", "kind": "detail",
                "parent_entity_id": repair.entity.id }),
    )
    .await;
    for f in [
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer" }),
        json!({ "field_key": "unit", "label": "單價", "data_type": "money", "decimal_places": 2 }),
        formula_field("amount", "{qty} * {unit}", "money"),
        formula_field("per", "{unit} / {qty}", "decimal"),
    ] {
        create_field(pool, &admin, PARTS, f).await;
    }

    for (key, formula, result) in [
        ("subtotal", "{qty} * {price}", "money"),
        ("parts_total", "SUM(parts.amount)", "money"),
        ("grand_total", "{subtotal} + {parts_total}", "money"),
        (
            "discounted",
            "{subtotal} * (1 - COALESCE({customer.discount}, 0))",
            "money",
        ),
        ("part_count", "COUNT(parts.amount)", "integer"),
        ("ratio", "{qty} / {divisor}", "decimal"),
        ("label", "{note} & \"-\" & {qty}", "text"),
        ("today", "TODAY()", "date"),
    ] {
        create_field(pool, &admin, REPAIR, formula_field(key, formula, result)).await;
    }
    admin
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

fn record_uri(entity: &str, id: Id) -> String {
    format!("{}/{id}", records_uri(entity))
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn update(pool: &PgPool, token: &str, entity: &str, id: Id, body: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::PATCH,
        &record_uri(entity, id),
        token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(entity, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn list(pool: &PgPool, token: &str, query: &str) -> Page<Record> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("{}?{query}", records_uri(REPAIR)),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 欄位的值；沒有值時為 `null`。
fn value(record: &Record, key: &str) -> Value {
    record.data.get(key).cloned().unwrap_or(Value::Null)
}

/// 主表某些欄位的值。
fn values(record: &Record, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .map(|k| ((*k).to_owned(), value(record, k)))
            .collect(),
    )
}

/// 明細某個欄位的值，依順序。
fn column(record: &Record, field: &str) -> Vec<Value> {
    record.lines.as_ref().unwrap()[PARTS]
        .iter()
        .map(|l| value(l, field))
        .collect()
}

/// 資料庫中的 `formula_errors`。
async fn stored_errors(pool: &PgPool, id: Id) -> Option<Value> {
    sqlx::query_scalar("SELECT formula_errors FROM custom_records WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// 最後一筆操作紀錄的 `(before_data, after_data)`。
async fn last_audit(pool: &PgPool, id: Id) -> (Option<Value>, Option<Value>) {
    sqlx::query_as(
        "SELECT before_data, after_data FROM audit_logs
         WHERE record_id = $1 ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

const MONEY_KEYS: &[&str] = &[
    "subtotal",
    "discounted",
    "parts_total",
    "grand_total",
    "part_count",
    "ratio",
    "label",
];

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

/// 新增資料後，本筆欄位、關聯引用、子表彙總的公式值正確；明細的公式先算，主表彙總使用明細的結果。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn computes_formulas_on_create(pool: PgPool) {
    let admin = setup(&pool).await;
    let customer = create(
        &pool,
        &admin,
        CUSTOMER,
        json!({ "data": { "name": "王先生", "discount": "0.10" } }),
    )
    .await;

    let before_day = today_taipei();
    let record = create(
        &pool,
        &admin,
        REPAIR,
        json!({
            "data": { "qty": 2, "price": "50", "divisor": 4, "note": "A", "customer": customer.id },
            "lines": { PARTS: [
                { "data": { "qty": 2, "unit": "10" } },
                { "data": { "qty": 4, "unit": "5.5" } },
            ] },
        }),
    )
    .await;
    let after_day = today_taipei();

    // 明細的公式
    assert_eq!(column(&record, "amount"), [json!("20.00"), json!("22.00")]);
    assert_eq!(column(&record, "per"), [json!("5.00"), json!("1.38")]);
    // 主表：本筆欄位、關聯引用、子表彙總（使用明細公式的結果）、引用其他公式的公式
    assert_eq!(
        values(&record, MONEY_KEYS),
        json!({
            "subtotal": "100.00",
            "discounted": "90.00",
            "parts_total": "42.00",
            "grand_total": "142.00",
            "part_count": 2,
            "ratio": "0.50",
            "label": "A-2",
        })
    );
    // TODAY() 為 Asia/Taipei 的日期
    let today = value(&record, "today");
    assert!(
        [before_day, after_day]
            .iter()
            .any(|d| today == json!(d.to_string())),
        "{today}"
    );
    assert!(record.formula_errors.is_empty());
    assert_eq!(stored_errors(&pool, record.id).await, None);

    // 讀取的結果相同
    assert_eq!(get(&pool, &admin, REPAIR, record.id).await, record);

    // 操作紀錄包含公式欄位的值
    let (_, after) = last_audit(&pool, record.id).await;
    let after = after.unwrap();
    assert_eq!(after["grand_total"], json!("142.00"));
    assert_eq!(after["lines.parts"][1]["data"]["amount"], json!("22.00"));
}

/// 修改明細後主表的彙總更新；只修改主表時，沒帶到的明細也重算，彙總維持正確。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn updating_lines_updates_master_aggregates(pool: PgPool) {
    let admin = setup(&pool).await;
    let record = create(
        &pool,
        &admin,
        REPAIR,
        json!({
            "data": { "qty": 1, "price": "10", "divisor": 1 },
            "lines": { PARTS: [
                { "data": { "qty": 1, "unit": "3" } },
                { "data": { "qty": 2, "unit": "4" } },
            ] },
        }),
    )
    .await;
    assert_eq!(value(&record, "parts_total"), json!("11.00"));
    let first = &record.lines.as_ref().unwrap()[PARTS][0];

    // 修改第一列、刪除第二列、新增一列
    let updated = update(
        &pool,
        &admin,
        REPAIR,
        record.id,
        json!({
            "version": record.version,
            "data": {},
            "lines": { PARTS: [
                { "id": first.id, "data": { "qty": 5 } },
                { "data": { "qty": 1, "unit": "100" } },
            ] },
        }),
    )
    .await;
    assert_eq!(
        column(&updated, "amount"),
        [json!("15.00"), json!("100.00")]
    );
    assert_eq!(
        values(&updated, &["parts_total", "grand_total", "part_count"]),
        json!({ "parts_total": "115.00", "grand_total": "125.00", "part_count": 2 })
    );
    assert_eq!(updated.version, record.version + 1);

    // 操作紀錄的 diff 包含公式欄位的變動
    let (before, after) = last_audit(&pool, record.id).await;
    let (before, after) = (before.unwrap(), after.unwrap());
    assert_eq!(before["parts_total"], json!("11.00"));
    assert_eq!(after["parts_total"], json!("115.00"));
    assert_eq!(after["grand_total"], json!("125.00"));
    assert!(
        before.get("subtotal").is_none(),
        "沒變動的公式不出現：{before}"
    );
    assert_eq!(after["lines.parts"][0]["data"]["amount"], json!("15.00"));

    // 只修改主表（沒帶到明細）：明細的公式重算但沒變動，主表的彙總維持正確
    let master_only = update(
        &pool,
        &admin,
        REPAIR,
        record.id,
        json!({ "version": updated.version, "data": { "price": "20" } }),
    )
    .await;
    assert_eq!(
        values(&master_only, &["subtotal", "parts_total", "grand_total"]),
        json!({ "subtotal": "20.00", "parts_total": "115.00", "grand_total": "135.00" })
    );
    assert_eq!(
        master_only.lines.as_ref().unwrap()[PARTS],
        updated.lines.as_ref().unwrap()[PARTS]
    );
    let (_, after) = last_audit(&pool, record.id).await;
    assert!(
        after.unwrap().get("lines.parts").is_none(),
        "明細沒有變動，不出現在 diff"
    );

    // 沒有任何變動：不寫入，version 不變
    let same = update(
        &pool,
        &admin,
        REPAIR,
        record.id,
        json!({ "version": master_only.version, "data": { "price": "20" } }),
    )
    .await;
    assert_eq!(same.version, master_only.version);
}

/// 除以零時存檔成功，該欄位沒有值，`formula_errors` 有原因；改正後錯誤清除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn division_by_zero_saves_with_error(pool: PgPool) {
    let admin = setup(&pool).await;
    let record = create(
        &pool,
        &admin,
        REPAIR,
        json!({
            "data": { "qty": 3, "price": "1", "divisor": 0 },
            "lines": { PARTS: [{ "data": { "qty": 0, "unit": "5" } }] },
        }),
    )
    .await;
    assert!(record.data.get("ratio").is_none(), "{:?}", record.data);
    assert_eq!(record.formula_errors.len(), 1, "{record:?}");
    assert_eq!(record.formula_errors["ratio"], "除數不可為零");
    // 其他公式不受影響
    assert_eq!(value(&record, "subtotal"), json!("3.00"));
    assert_eq!(
        stored_errors(&pool, record.id).await,
        Some(json!({ "ratio": "除數不可為零" }))
    );

    // 明細的錯誤記在該列
    let line = &record.lines.as_ref().unwrap()[PARTS][0];
    assert!(line.data.get("per").is_none());
    assert_eq!(line.formula_errors["per"], "除數不可為零");
    assert_eq!(value(line, "amount"), json!("0.00"));

    // 讀取與列表都帶 formula_errors
    let read = get(&pool, &admin, REPAIR, record.id).await;
    assert_eq!(read.formula_errors, record.formula_errors);
    assert_eq!(
        read.lines.as_ref().unwrap()[PARTS][0].formula_errors,
        line.formula_errors
    );
    let page = list(&pool, &admin, "").await;
    assert_eq!(page.items[0].formula_errors, record.formula_errors);

    // 改正後錯誤清除
    let fixed = update(
        &pool,
        &admin,
        REPAIR,
        record.id,
        json!({
            "version": record.version,
            "data": { "divisor": 2 },
            "lines": { PARTS: [{ "id": line.id, "data": { "qty": 2 } }] },
        }),
    )
    .await;
    assert_eq!(value(&fixed, "ratio"), json!("1.50"));
    assert!(fixed.formula_errors.is_empty());
    assert_eq!(stored_errors(&pool, record.id).await, None);
    let fixed_line = &fixed.lines.as_ref().unwrap()[PARTS][0];
    assert_eq!(value(fixed_line, "per"), json!("2.50"));
    assert!(fixed_line.formula_errors.is_empty());
    assert_eq!(stored_errors(&pool, line.id).await, None);
}

/// request 帶了公式欄位的值時忽略，不回傳錯誤。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn formula_values_in_request_are_ignored(pool: PgPool) {
    let admin = setup(&pool).await;
    let record = create(
        &pool,
        &admin,
        REPAIR,
        json!({
            "data": { "qty": 1, "price": "2", "subtotal": "999", "ratio": "abc", "today": 1 },
            "lines": { PARTS: [{ "data": { "qty": 1, "unit": "3", "amount": "999" } }] },
        }),
    )
    .await;
    assert_eq!(value(&record, "subtotal"), json!("2.00"));
    assert_eq!(column(&record, "amount"), [json!("3.00")]);
    // divisor 沒有值 → ratio 為空值（不是錯誤）
    assert!(record.data.get("ratio").is_none());
    assert!(record.formula_errors.is_empty());

    // 修改時也忽略；沒有其他變動時不寫入
    let same = update(
        &pool,
        &admin,
        REPAIR,
        record.id,
        json!({ "version": record.version, "data": { "subtotal": "5" } }),
    )
    .await;
    assert_eq!(value(&same, "subtotal"), json!("2.00"));
    assert_eq!(same.version, record.version, "{record:?}\n{same:?}");
}

/// 依公式欄位排序、篩選（依 `formula_result_type`）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sort_and_filter_by_formula(pool: PgPool) {
    let admin = setup(&pool).await;
    let mut ids = Vec::new();
    // subtotal：20、3、100；ratio：2、除以零（沒有值）、2.5
    for (qty, price, divisor, note) in [(2, "10", 1, "B"), (3, "1", 0, "A"), (10, "10", 4, "AB")] {
        let r = create(
            &pool,
            &admin,
            REPAIR,
            json!({ "data": { "qty": qty, "price": price, "divisor": divisor, "note": note } }),
        )
        .await;
        ids.push(r.id);
    }
    let order = |page: Page<Record>| page.items.iter().map(|r| r.id).collect::<Vec<_>>();

    // 依數值排序（不是字串："100.00" < "20.00"）
    assert_eq!(
        order(list(&pool, &admin, "sort=-subtotal").await),
        [ids[2], ids[0], ids[1]]
    );
    assert_eq!(
        order(list(&pool, &admin, "sort=subtotal").await),
        [ids[1], ids[0], ids[2]]
    );
    // 沒有值的排在最後
    assert_eq!(
        order(list(&pool, &admin, "sort=ratio").await),
        [ids[0], ids[2], ids[1]]
    );

    assert_eq!(
        order(list(&pool, &admin, "filter.subtotal.gte=20").await),
        [ids[2], ids[0]]
    );
    assert_eq!(
        order(list(&pool, &admin, "filter.ratio.empty=true").await),
        [ids[1]]
    );
    assert_eq!(
        order(list(&pool, &admin, "filter.label.contains=a-").await),
        [ids[1]]
    );
    assert_eq!(
        order(list(&pool, &admin, "filter.part_count.eq=0").await).len(),
        3
    );

    // 運算子依結果型態檢查
    for query in [
        "filter.label.gt=a",
        "filter.subtotal.contains=1",
        "sort=customer",
    ] {
        let (status, body) = call(
            &pool,
            Method::GET,
            &format!("{}?{query}", records_uri(REPAIR)),
            &admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}: {body}");
        let err: ApiError = serde_json::from_value(body).unwrap();
        assert_eq!(err.code, ErrorCode::Validation);
    }
}

/// 關聯目標修改後，引用它的資料不變；該筆資料再次存檔後更新。目標已刪除時仍讀取其資料。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn relation_changes_apply_on_next_save(pool: PgPool) {
    let admin = setup(&pool).await;
    let customer = create(
        &pool,
        &admin,
        CUSTOMER,
        json!({ "data": { "name": "王先生", "discount": "0.10" } }),
    )
    .await;
    let record = create(
        &pool,
        &admin,
        REPAIR,
        json!({ "data": { "qty": 1, "price": "100", "customer": customer.id } }),
    )
    .await;
    assert_eq!(value(&record, "discounted"), json!("90.00"));

    // 修改客戶的折扣：維修單不變
    let customer = update(
        &pool,
        &admin,
        CUSTOMER,
        customer.id,
        json!({ "version": customer.version, "data": { "discount": "0.20" } }),
    )
    .await;
    let unchanged = get(&pool, &admin, REPAIR, record.id).await;
    assert_eq!(value(&unchanged, "discounted"), json!("90.00"));
    assert_eq!(unchanged.version, record.version);

    // 維修單再次存檔（修改其他欄位）→ 讀取目前的折扣
    let resaved = update(
        &pool,
        &admin,
        REPAIR,
        record.id,
        json!({ "version": record.version, "data": { "note": "再次存檔" } }),
    )
    .await;
    assert_eq!(value(&resaved, "discounted"), json!("80.00"));
    let (before, after) = last_audit(&pool, record.id).await;
    assert_eq!(before.unwrap()["discounted"], json!("90.00"));
    assert_eq!(after.unwrap()["discounted"], json!("80.00"));

    // 客戶改了折扣後被刪除：仍讀取已刪除資料的值
    let customer = update(
        &pool,
        &admin,
        CUSTOMER,
        customer.id,
        json!({ "version": customer.version, "data": { "discount": "0.50" } }),
    )
    .await;
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!(
            "{}?version={}",
            record_uri(CUSTOMER, customer.id),
            customer.version
        ),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let after_delete = update(
        &pool,
        &admin,
        REPAIR,
        record.id,
        json!({ "version": resaved.version, "data": { "note": "目標已刪除" } }),
    )
    .await;
    assert_eq!(value(&after_delete, "discounted"), json!("50.00"));
    assert!(after_delete.formula_errors.is_empty());
}
