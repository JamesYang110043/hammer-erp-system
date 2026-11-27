//! 客戶品名批次寫入 API（docs/architecture.md §8.9「客戶品名批次寫入 API」，M12）。
//!
//! Excel 匯入與開單時存成客戶品名共用：依（客戶，商品）upsert 客戶品名，結果與經由客戶的通用資料 API
//! 存檔相同（§8.7 的檢查、客戶的 `version`、操作紀錄）。
//!
//! 流程：
//!
//! 1. 每列的格式檢查（[`check_row`]）：指定方式（編號或 id，二擇一）、編號空白、品名（去除前後空白、
//!    必填、長度、控制字元）
//! 2. 解析客戶、商品（編號不分大小寫，與唯一索引相同；停用的可以，已刪除的不行）
//! 3. 同一批中（客戶，商品）重複 → 第二列以後錯誤
//! 4. 比對既有的客戶品名決定動作（新增、修改、不變）
//! 5. `dry_run = false`：在一個 transaction 內先鎖定客戶（`FOR UPDATE`）、商品（`FOR SHARE`），以鎖定後的
//!    資料重新執行 2–4；沒有錯誤才寫入。每位有變動的客戶 `version` + 1，並寫入一筆操作紀錄
//!    （`lines.customer_product_name` 的修改前後，格式與客戶 API 相同）

use std::collections::{HashMap, HashSet};

use hammer_db::DbError;
use hammer_db::sales::{self as db, CustomerProductNameRow};
use hammer_db::sqlx::{PgConnection, PgPool};
use hammer_shared::sales::customer_product_name_batch::{MAX_ROWS, ROWS};
use hammer_shared::sales::{
    CustomerProductNameBatchAction as Action, CustomerProductNameBatchError as RowError,
    CustomerProductNameBatchRequest, CustomerProductNameBatchResult, CustomerProductNameBatchRow,
    CustomerProductNameBatchRowResult, CustomerProductNameBatchSummary,
};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::customer_names::MSG_PRODUCT_NOT_FOUND;
use super::product_lookup::MSG_CUSTOMER_NOT_FOUND;
use crate::CoreError;
use crate::audit::AuditDiff;
use crate::auth::AuthContext;
use crate::master_data::{
    CUSTOMER, CUSTOMER_PRODUCT_NAME, CUSTOMER_PRODUCT_NAME_INDEX,
    MSG_CUSTOMER_PRODUCT_NAME_CONTROL, MSG_CUSTOMER_PRODUCT_NAME_TAKEN, MSG_REQUIRED,
};
use crate::record_lines::{audit_key, audit_value};
use crate::records::{
    commit, detail_lines, load_details_for_write, load_entity_for_write, record_audit,
};

/// 指定方式不正確（兩種都帶、都沒帶、混用）。
pub const MSG_TARGET: &str = "請以編號或 id 指定客戶與商品（二擇一）";
/// 客戶編號空白。
pub const MSG_CUSTOMER_CODE_BLANK: &str = "客戶編號不可空白";
/// 料號空白。
pub const MSG_PRODUCT_CODE_BLANK: &str = "料號不可空白";
/// 客戶品名的預設長度上限（欄位定義的 `max_length`）。
const DEFAULT_NAME_MAX: usize = 200;

/// 列數不在 1–[`MAX_ROWS`]。
fn rows_error(len: usize) -> CoreError {
    CoreError::InvalidField {
        field: ROWS,
        message: if len == 0 {
            "請至少提供一列".to_owned()
        } else {
            format!("一次最多 {MAX_ROWS} 列，這次有 {len} 列")
        },
    }
}

/// 一列指定客戶、商品的方式。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// 編號（去除前後空白、轉小寫後比對）。
    Codes {
        customer: String,
        product: String,
    },
    Ids {
        customer: Id,
        product: Id,
    },
}

/// 格式檢查後的一列。
#[derive(Debug, Clone)]
struct Parsed {
    target: Option<Target>,
    /// 原始的編號（錯誤訊息用）。
    customer_code: String,
    product_code: String,
    name: Option<String>,
    /// `None`：不更新；`Some(None)`：清空。
    notes: Option<Option<String>>,
    errors: Vec<RowError>,
}

fn error(field: Option<&str>, message: impl Into<String>) -> RowError {
    RowError {
        field: field.map(str::to_owned),
        message: message.into(),
        duplicate_of: None,
    }
}

/// 一列的格式檢查（不需要資料庫）。
fn check_row(row: &CustomerProductNameBatchRow, name_max: usize) -> Parsed {
    let mut errors = Vec::new();
    let has_codes = row.customer_code.is_some() || row.product_code.is_some();
    let has_ids = row.customer_id.is_some() || row.product_id.is_some();
    let customer_code = row.customer_code.as_deref().unwrap_or_default().trim();
    let product_code = row.product_code.as_deref().unwrap_or_default().trim();
    let target = match (has_codes, has_ids) {
        (true, false) if row.customer_code.is_some() && row.product_code.is_some() => {
            if customer_code.is_empty() {
                errors.push(error(Some("customer_code"), MSG_CUSTOMER_CODE_BLANK));
            }
            if product_code.is_empty() {
                errors.push(error(Some("product_code"), MSG_PRODUCT_CODE_BLANK));
            }
            (!customer_code.is_empty() && !product_code.is_empty()).then(|| Target::Codes {
                customer: customer_code.to_lowercase(),
                product: product_code.to_lowercase(),
            })
        }
        (false, true) => match (row.customer_id, row.product_id) {
            (Some(customer), Some(product)) => Some(Target::Ids { customer, product }),
            _ => {
                errors.push(error(None, MSG_TARGET));
                None
            }
        },
        _ => {
            errors.push(error(None, MSG_TARGET));
            None
        }
    };

    let raw = row.name.as_deref().unwrap_or_default();
    let trimmed = raw.trim();
    let name = if trimmed.is_empty() {
        errors.push(error(Some("name"), MSG_REQUIRED));
        None
    } else if trimmed.chars().count() > name_max {
        errors.push(error(Some("name"), format!("不可超過 {name_max} 個字元")));
        None
    } else if trimmed.chars().any(char::is_control) {
        errors.push(error(Some("name"), MSG_CUSTOMER_PRODUCT_NAME_CONTROL));
        None
    } else {
        Some(trimmed.to_owned())
    };
    let notes = row.notes.as_ref().map(|n| {
        n.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    });
    Parsed {
        target,
        customer_code: customer_code.to_owned(),
        product_code: product_code.to_owned(),
        name,
        notes,
        errors,
    }
}

/// 一列的評估結果。
struct Evaluated {
    customer: Option<Id>,
    product: Option<Id>,
    action: Option<Action>,
    errors: Vec<RowError>,
    /// 要更新的既有客戶品名。
    existing: Option<CustomerProductNameRow>,
}

/// 解析客戶、商品並決定每列的動作。`lock = true`（寫入）時鎖定客戶與商品，必須在 transaction 內。
async fn evaluate(
    conn: &mut PgConnection,
    parsed: &[Parsed],
    lock: bool,
) -> Result<Vec<Evaluated>, CoreError> {
    // 編號 → id
    let mut customer_codes: Vec<String> = Vec::new();
    let mut product_codes: Vec<String> = Vec::new();
    for p in parsed {
        if let Some(Target::Codes { customer, product }) = &p.target {
            customer_codes.push(customer.clone());
            product_codes.push(product.clone());
        }
    }
    customer_codes.sort_unstable();
    customer_codes.dedup();
    product_codes.sort_unstable();
    product_codes.dedup();
    let customer_by_code: HashMap<String, Id> = if customer_codes.is_empty() {
        HashMap::new()
    } else {
        db::customers_by_codes(&mut *conn, &customer_codes)
            .await?
            .into_iter()
            .collect()
    };
    let product_by_code: HashMap<String, Id> = if product_codes.is_empty() {
        HashMap::new()
    } else {
        db::products_by_codes(&mut *conn, &product_codes)
            .await?
            .into_iter()
            .collect()
    };
    let resolved: Vec<(Option<Id>, Option<Id>)> = parsed
        .iter()
        .map(|p| match &p.target {
            Some(Target::Codes { customer, product }) => (
                customer_by_code.get(customer).copied(),
                product_by_code.get(product).copied(),
            ),
            Some(Target::Ids { customer, product }) => (Some(*customer), Some(*product)),
            None => (None, None),
        })
        .collect();

    // id 是否存在（寫入時一併鎖定）
    let mut customers: Vec<Id> = resolved.iter().filter_map(|r| r.0).collect();
    customers.sort_unstable();
    customers.dedup();
    let mut products: Vec<Id> = resolved.iter().filter_map(|r| r.1).collect();
    products.sort_unstable();
    products.dedup();
    let live_customers: HashSet<Id> = match (customers.is_empty(), lock) {
        (true, _) => Vec::new(),
        (false, true) => db::lock_customers_for_update(&mut *conn, &customers).await?,
        (false, false) => db::existing_customers(&mut *conn, &customers).await?,
    }
    .into_iter()
    .collect();
    let live_products: HashSet<Id> = match (products.is_empty(), lock) {
        (true, _) => Vec::new(),
        (false, true) => db::lock_products_for_share(&mut *conn, &products).await?,
        (false, false) => db::existing_products(&mut *conn, &products).await?,
    }
    .into_iter()
    .collect();

    let live: Vec<Id> = live_customers.iter().copied().collect();
    let existing: HashMap<(Id, Id), CustomerProductNameRow> = if live.is_empty() {
        HashMap::new()
    } else {
        db::customer_product_names_of(&mut *conn, &live)
            .await?
            .into_iter()
            .map(|r| ((r.customer_id, r.product_id), r))
            .collect()
    };

    let mut first_of: HashMap<(Id, Id), usize> = HashMap::new();
    let mut out = Vec::with_capacity(parsed.len());
    for (index, (p, (customer, product))) in parsed.iter().zip(resolved).enumerate() {
        let mut errors = p.errors.clone();
        let customer = customer.filter(|c| live_customers.contains(c));
        let product = product.filter(|c| live_products.contains(c));
        match &p.target {
            Some(Target::Codes { .. }) => {
                if customer.is_none() {
                    errors.push(error(
                        Some("customer_code"),
                        format!("找不到客戶編號 {}", p.customer_code),
                    ));
                }
                if product.is_none() {
                    errors.push(error(
                        Some("product_code"),
                        format!("找不到料號 {}", p.product_code),
                    ));
                }
            }
            Some(Target::Ids { .. }) => {
                if customer.is_none() {
                    errors.push(error(Some("customer_id"), MSG_CUSTOMER_NOT_FOUND));
                }
                if product.is_none() {
                    errors.push(error(Some("product_id"), MSG_PRODUCT_NOT_FOUND));
                }
            }
            None => {}
        }
        if let (Some(c), Some(pr)) = (customer, product) {
            if let Some(first) = first_of.get(&(c, pr)) {
                errors.push(RowError {
                    field: None,
                    message: format!("與第 {} 列重複（同一客戶、同一商品）", first + 1),
                    duplicate_of: Some(*first as u32),
                });
            } else {
                first_of.insert((c, pr), index);
            }
        }
        let (action, current) = match (errors.is_empty(), customer, product, &p.name) {
            (true, Some(c), Some(pr), Some(name)) => match existing.get(&(c, pr)) {
                None => (Some(Action::Create), None),
                Some(row) => {
                    let notes_changed = p.notes.as_ref().is_some_and(|n| *n != row.notes);
                    if row.name != *name || notes_changed {
                        (Some(Action::Update), Some(row.clone()))
                    } else {
                        (Some(Action::Unchanged), None)
                    }
                }
            },
            _ => (None, None),
        };
        out.push(Evaluated {
            customer,
            product,
            action,
            errors,
            existing: current,
        });
    }
    Ok(out)
}

fn result_of(dry_run: bool, evaluated: &[Evaluated]) -> CustomerProductNameBatchResult {
    let mut summary = CustomerProductNameBatchSummary::default();
    let rows = evaluated
        .iter()
        .enumerate()
        .map(|(index, e)| {
            match e.action {
                Some(Action::Create) => summary.create += 1,
                Some(Action::Update) => summary.update += 1,
                Some(Action::Unchanged) => summary.unchanged += 1,
                None => summary.error += 1,
            }
            CustomerProductNameBatchRowResult {
                index: index as u32,
                action: e.action,
                customer_id: e.customer,
                product_id: e.product,
                errors: e.errors.clone(),
            }
        })
        .collect();
    CustomerProductNameBatchResult {
        dry_run,
        summary,
        rows,
    }
}

/// 有錯誤時整批不寫入：`422 VALIDATION`，`details` 為每列的結果。
fn rejected(result: &CustomerProductNameBatchResult) -> Result<CoreError, CoreError> {
    Ok(CoreError::ValidationDetails {
        message: format!("有 {} 列錯誤，未寫入", result.summary.error),
        details: serde_json::to_value(result)
            .map_err(|e| CoreError::Internal(format!("無法序列化批次寫入結果：{e}")))?,
    })
}

/// `POST /api/v1/sales/customer-product-names/batch`。需要 `record.write`。
///
/// - `rows` 為 0 列或超過 [`MAX_ROWS`] 列 → [`CoreError::InvalidField`]（`rows`）
/// - `dry_run = false` 且有錯誤的列 → [`CoreError::ValidationDetails`]（整批不寫入）
pub async fn batch_write(
    pool: &PgPool,
    ctx: &AuthContext,
    req: &CustomerProductNameBatchRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<CustomerProductNameBatchResult, CoreError> {
    ctx.require(permission::RECORD_WRITE)?;
    if req.rows.is_empty() || req.rows.len() > MAX_ROWS {
        return Err(rows_error(req.rows.len()));
    }

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, _, _) = load_entity_for_write(&mut tx, CUSTOMER).await?;
    let details = load_details_for_write(&mut tx, &entity).await?;
    let detail = details
        .iter()
        .find(|d| d.entity.key == CUSTOMER_PRODUCT_NAME)
        .ok_or_else(|| CoreError::Internal("找不到客戶品名的明細子表".into()))?;
    let name_max = detail
        .fields
        .iter()
        .find(|f| f.field_key == "name")
        .and_then(|f| f.validation.as_ref())
        .and_then(|v| v.max_length)
        .map_or(DEFAULT_NAME_MAX, |m| m as usize);
    let parsed: Vec<Parsed> = req.rows.iter().map(|r| check_row(r, name_max)).collect();

    if req.dry_run {
        // 只讀取，不鎖定
        let evaluated = evaluate(&mut tx, &parsed, false).await?;
        tx.rollback().await.map_err(DbError::from)?;
        return Ok(result_of(true, &evaluated));
    }

    let evaluated = evaluate(&mut tx, &parsed, true).await?;
    let result = result_of(false, &evaluated);
    if result.summary.error > 0 {
        tx.rollback().await.map_err(DbError::from)?;
        return Err(rejected(&result)?);
    }

    // 依客戶分組（保持 rows 的順序），只處理有變動的客戶
    let mut by_customer: Vec<(Id, Vec<usize>)> = Vec::new();
    for (i, e) in evaluated.iter().enumerate() {
        if !matches!(e.action, Some(Action::Create | Action::Update)) {
            continue;
        }
        let customer = e.customer.unwrap_or_default();
        match by_customer.iter_mut().find(|(c, _)| *c == customer) {
            Some((_, rows)) => rows.push(i),
            None => by_customer.push((customer, vec![i])),
        }
    }
    let audit_key = audit_key(CUSTOMER_PRODUCT_NAME);
    for (customer, rows) in &by_customer {
        let before = lines_audit(&mut tx, detail, *customer).await?;
        let mut next_sort = next_sort_order(&before);
        for &i in rows {
            let (e, p) = (&evaluated[i], &parsed[i]);
            let name = p.name.as_deref().unwrap_or_default();
            let written = match &e.existing {
                None => {
                    let notes = p.notes.clone().flatten();
                    let sort_order = next_sort;
                    next_sort += 1;
                    db::insert_customer_product_name(
                        &mut *tx,
                        new_id(),
                        *customer,
                        sort_order,
                        e.product.unwrap_or_default(),
                        name,
                        notes.as_deref(),
                        ctx.user_id,
                    )
                    .await
                }
                Some(row) => {
                    let notes = p.notes.clone().unwrap_or_else(|| row.notes.clone());
                    db::update_customer_product_name(
                        &mut *tx,
                        row.id,
                        name,
                        notes.as_deref(),
                        ctx.user_id,
                    )
                    .await
                }
            };
            if let Err(err) = written {
                if err.unique_violation() == Some(CUSTOMER_PRODUCT_NAME_INDEX) {
                    // 與同時進行的其他存檔衝突
                    tx.rollback().await.map_err(DbError::from)?;
                    return Err(taken(&parsed, evaluated, i)?);
                }
                return Err(err.into());
            }
        }
        db::touch_customer(&mut *tx, *customer, ctx.user_id).await?;
        let after = lines_audit(&mut tx, detail, *customer).await?;
        let before = Value::Object(Map::from_iter([(audit_key.clone(), before.1)]));
        let after = Value::Object(Map::from_iter([(audit_key.clone(), after.1)]));
        record_audit(
            &mut tx,
            ctx,
            action::UPDATE,
            CUSTOMER,
            *customer,
            AuditDiff::between(Some(&before), Some(&after))?,
            client_info,
            now,
        )
        .await?;
    }
    commit(tx).await?;
    tracing::info!(
        actor = %ctx.user_id,
        customers = by_customer.len(),
        create = result.summary.create,
        update = result.summary.update,
        "已批次寫入客戶品名"
    );
    Ok(result)
}

/// 客戶 `customer` 目前的客戶品名：`(最大的 sort_order, 操作紀錄的值)`。
async fn lines_audit(
    conn: &mut PgConnection,
    detail: &crate::records::Detail,
    customer: Id,
) -> Result<(Option<i32>, Value), CoreError> {
    let lines = detail_lines(conn, std::slice::from_ref(detail), customer)
        .await?
        .pop()
        .unwrap_or_default();
    let max = lines.iter().map(|l| l.sort_order).max();
    Ok((max, audit_value(lines.iter().map(|l| (l.id, &l.data)))))
}

/// 新增的客戶品名排在既有的之後。
fn next_sort_order(before: &(Option<i32>, Value)) -> i32 {
    before.0.map_or(0, |m| m + 1)
}

/// 寫入時違反唯一索引：第 `index` 列加上「此商品已設定客戶品名」，整批不寫入。
fn taken(
    parsed: &[Parsed],
    mut evaluated: Vec<Evaluated>,
    index: usize,
) -> Result<CoreError, CoreError> {
    let field = match parsed[index].target {
        Some(Target::Codes { .. }) => "product_code",
        _ => "product_id",
    };
    let row = &mut evaluated[index];
    row.action = None;
    row.errors
        .push(error(Some(field), MSG_CUSTOMER_PRODUCT_NAME_TAKEN));
    rejected(&result_of(false, &evaluated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(json: Value) -> CustomerProductNameBatchRow {
        serde_json::from_value(json).unwrap()
    }

    fn messages(p: &Parsed) -> Vec<(Option<&str>, &str)> {
        p.errors
            .iter()
            .map(|e| (e.field.as_deref(), e.message.as_str()))
            .collect()
    }

    #[test]
    fn codes_or_ids_but_not_both() {
        let id = "01a211f5-20e9-7b08-9d17-2a0dbce98c12";
        let ok = check_row(
            &row(
                serde_json::json!({ "customer_code": " C001 ", "product_code": "p001", "name": " X " }),
            ),
            200,
        );
        assert!(ok.errors.is_empty());
        assert_eq!(
            ok.target,
            Some(Target::Codes {
                customer: "c001".into(),
                product: "p001".into()
            })
        );
        assert_eq!(ok.name.as_deref(), Some("X"));
        let ids = check_row(
            &row(serde_json::json!({ "customer_id": id, "product_id": id, "name": "X" })),
            200,
        );
        assert!(matches!(ids.target, Some(Target::Ids { .. })));
        for bad in [
            serde_json::json!({ "name": "X" }),
            serde_json::json!({ "customer_code": "C1", "product_id": id, "name": "X" }),
            serde_json::json!({ "customer_code": "C1", "name": "X" }),
            serde_json::json!({ "customer_id": id, "name": "X" }),
            serde_json::json!({ "customer_code": "C1", "product_code": "P1",
                                "customer_id": id, "product_id": id, "name": "X" }),
        ] {
            let p = check_row(&row(bad.clone()), 200);
            assert_eq!(messages(&p), [(None, MSG_TARGET)], "{bad}");
            assert!(p.target.is_none());
        }
        let blank = check_row(
            &row(serde_json::json!({ "customer_code": " ", "product_code": "", "name": "X" })),
            200,
        );
        assert_eq!(
            messages(&blank),
            [
                (Some("customer_code"), MSG_CUSTOMER_CODE_BLANK),
                (Some("product_code"), MSG_PRODUCT_CODE_BLANK)
            ]
        );
    }

    #[test]
    fn name_rules_match_customer_api() {
        let base = |name: Value| {
            check_row(
                &row(
                    serde_json::json!({ "customer_code": "C", "product_code": "P", "name": name }),
                ),
                5,
            )
        };
        assert_eq!(messages(&base(Value::Null)), [(Some("name"), MSG_REQUIRED)]);
        assert_eq!(messages(&base("  ".into())), [(Some("name"), MSG_REQUIRED)]);
        assert_eq!(
            messages(&base("扳手扳手扳手".into())),
            [(Some("name"), "不可超過 5 個字元")]
        );
        assert!(base(" 扳手扳手扳 ".into()).errors.is_empty());
        assert_eq!(
            messages(&base("a\tb".into())),
            [(Some("name"), MSG_CUSTOMER_PRODUCT_NAME_CONTROL)]
        );
    }

    #[test]
    fn notes_absent_null_or_text() {
        let p = |json: Value| check_row(&row(json), 200).notes;
        let base = serde_json::json!({ "customer_code": "C", "product_code": "P", "name": "X" });
        assert_eq!(p(base.clone()), None);
        let mut with = base.clone();
        with["notes"] = Value::Null;
        assert_eq!(p(with.clone()), Some(None));
        with["notes"] = "  ".into();
        assert_eq!(p(with.clone()), Some(None));
        with["notes"] = " 圖面 ".into();
        assert_eq!(p(with), Some(Some("圖面".into())));
    }
}
