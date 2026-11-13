//! 單號規則取號的資料庫整合測試（#68）：`hammer_core::records::create_record` 依資料類別的
//! `numbering_pattern` 產生 `record_no`。
//!
//! 直接呼叫 core 以指定建立時間（測試跨月、Asia/Taipei 日期）。
//! 使用 `#[sqlx::test]`，執行前需設定 `DATABASE_URL`，見 README「整合測試」。

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeSet, HashSet};

use hammer_core::CoreError;
use hammer_core::auth::AuthContext;
use hammer_core::records::{self, RECORD_NO_TAKEN};
use hammer_core::schema;
use hammer_db::sqlx::{self, PgPool};
use hammer_shared::record::{CreateRecordRequest, Record};
use hammer_shared::schema::{
    CreateEntityRequest, CreateFieldRequest, EntityKind, UpdateEntityRequest,
};
use hammer_shared::{new_id, permission, role};
use serde_json::json;
use time::OffsetDateTime;
use time::macros::datetime;

const ENTITY: &str = "repair_order";
/// 台北 2026-11-13 10:00。
const NOV: OffsetDateTime = datetime!(2026-11-13 02:00 UTC);

/// 新增一個管理者，回傳擁有所有權限的 [`AuthContext`]。
async fn admin(pool: &PgPool) -> AuthContext {
    let id = new_id();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, role_id)
         SELECT $1, 'admin', 'admin', '!', id FROM roles WHERE code = $2",
    )
    .bind(id)
    .bind(role::ADMIN)
    .execute(pool)
    .await
    .unwrap();
    AuthContext {
        user_id: id,
        username: "admin".into(),
        display_name: "admin".into(),
        role: role::ADMIN.into(),
        permissions: permission::ALL.iter().map(|p| (*p).to_owned()).collect(),
        session_id: new_id(),
        must_change_password: false,
    }
}

/// 建立資料類別 [`ENTITY`]（單號規則 `pattern`）與一個不可重複的文字欄位 `code`。
async fn setup(pool: &PgPool, pattern: Option<&str>) -> AuthContext {
    let ctx = admin(pool).await;
    schema::create_entity(
        pool,
        &ctx,
        &CreateEntityRequest {
            key: ENTITY.into(),
            label: "維修單".into(),
            icon: None,
            kind: EntityKind::Master,
            numbering_pattern: pattern.map(str::to_owned),
            title_template: None,
            sort_order: 0,
        },
        None,
        NOV,
    )
    .await
    .unwrap();
    let field: CreateFieldRequest = serde_json::from_value(json!({
        "field_key": "code", "label": "代碼", "data_type": "text", "is_unique": true,
    }))
    .unwrap();
    schema::create_field(pool, &ctx, ENTITY, &field, None, NOV)
        .await
        .unwrap();
    ctx
}

async fn create_at(
    pool: &PgPool,
    ctx: &AuthContext,
    code: &str,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    let req = CreateRecordRequest {
        data: json!({ "code": code }).as_object().unwrap().clone(),
    };
    records::create_record(pool, ctx, ENTITY, &req, None, now).await
}

async fn create_no(pool: &PgPool, ctx: &AuthContext, code: &str, now: OffsetDateTime) -> String {
    create_at(pool, ctx, code, now)
        .await
        .unwrap()
        .record_no
        .unwrap()
}

async fn set_pattern(pool: &PgPool, ctx: &AuthContext, pattern: Option<&str>) {
    let current = schema::get_entity(pool, ctx, ENTITY).await.unwrap().entity;
    let detail = schema::update_entity(
        pool,
        ctx,
        ENTITY,
        &UpdateEntityRequest {
            version: current.version,
            key: None,
            label: current.label,
            icon: current.icon,
            numbering_pattern: pattern.map(str::to_owned),
            title_template: current.title_template,
            sort_order: current.sort_order,
            is_active: current.is_active,
        },
        None,
        NOV,
    )
    .await
    .unwrap();
    assert_eq!(detail.entity.numbering_pattern.as_deref(), pattern);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn sequential_in_period_and_resets_next_month(pool: PgPool) {
    let ctx = setup(&pool, Some("RO{yyyyMM}-{000}")).await;

    for (i, expected) in ["RO202611-001", "RO202611-002", "RO202611-003"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            create_no(&pool, &ctx, &format!("n{i}"), NOV).await,
            expected
        );
    }

    // UTC 11/30 16:30 已是台北 12/01：新的期間從 1 開始
    let dec = datetime!(2026-11-30 16:30 UTC);
    assert_eq!(create_no(&pool, &ctx, "d1", dec).await, "RO202612-001");
    assert_eq!(create_no(&pool, &ctx, "d2", dec).await, "RO202612-002");
    // 台北 11/30 23:59 仍屬 11 月，接續 11 月的流水號
    let nov_end = datetime!(2026-11-30 15:59 UTC);
    assert_eq!(create_no(&pool, &ctx, "n9", nov_end).await, "RO202611-004");

    let periods: Vec<(String, i32)> = sqlx::query_as(
        "SELECT period_key, current_value FROM number_sequences ORDER BY period_key",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        periods,
        vec![("202611".to_owned(), 4), ("202612".to_owned(), 2)]
    );
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn without_date_codes_never_resets_and_overflows(pool: PgPool) {
    let ctx = setup(&pool, Some("RO-{000}")).await;
    assert_eq!(create_no(&pool, &ctx, "a", NOV).await, "RO-001");
    // 跨年也不重置
    let next_year = datetime!(2027-01-02 00:00 UTC);
    assert_eq!(create_no(&pool, &ctx, "b", next_year).await, "RO-002");

    // 超過位數時繼續遞增，不截斷
    sqlx::query("UPDATE number_sequences SET current_value = 999 WHERE period_key = ''")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(create_no(&pool, &ctx, "c", NOV).await, "RO-1000");
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn concurrent_creates_get_distinct_numbers(pool: PgPool) {
    const N: usize = 20;
    let ctx = setup(&pool, Some("RO{yyyyMM}-{0000}")).await;

    let tasks: Vec<_> = (0..N)
        .map(|i| {
            let (pool, ctx) = (pool.clone(), ctx.clone());
            tokio::spawn(async move { create_no(&pool, &ctx, &format!("c{i}"), NOV).await })
        })
        .collect();
    let mut numbers = BTreeSet::new();
    for task in tasks {
        numbers.insert(task.await.unwrap());
    }
    let expected: BTreeSet<String> = (1..=N).map(|i| format!("RO202611-{i:04}")).collect();
    assert_eq!(numbers, expected);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn changing_pattern_keeps_existing_numbers(pool: PgPool) {
    let ctx = setup(&pool, Some("A-{000}")).await;
    let first = create_at(&pool, &ctx, "a1", NOV).await.unwrap();
    assert_eq!(first.record_no.as_deref(), Some("A-001"));

    set_pattern(&pool, &ctx, Some("B{yy}{MM}-{0000}")).await;
    assert_eq!(create_no(&pool, &ctx, "b1", NOV).await, "B2611-0001");

    // 移除規則後新增的資料沒有單號
    set_pattern(&pool, &ctx, None).await;
    let plain = create_at(&pool, &ctx, "p1", NOV).await.unwrap();
    assert_eq!(plain.record_no, None);

    // 改回原規則：接續原本的流水號
    set_pattern(&pool, &ctx, Some("A-{000}")).await;
    assert_eq!(create_no(&pool, &ctx, "a2", NOV).await, "A-002");

    // 既有資料的單號不變
    let reloaded = records::get_record(&pool, &ctx, ENTITY, first.id)
        .await
        .unwrap();
    assert_eq!(reloaded.record_no.as_deref(), Some("A-001"));
    let reloaded = records::get_record(&pool, &ctx, ENTITY, plain.id)
        .await
        .unwrap();
    assert_eq!(reloaded.record_no, None);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn deleted_numbers_are_not_reused(pool: PgPool) {
    let ctx = setup(&pool, Some("A-{000}")).await;
    let first = create_at(&pool, &ctx, "a1", NOV).await.unwrap();
    let second = create_at(&pool, &ctx, "a2", NOV).await.unwrap();
    for r in [&first, &second] {
        records::delete_record(&pool, &ctx, ENTITY, r.id, r.version, None, NOV)
            .await
            .unwrap();
    }
    assert_eq!(create_no(&pool, &ctx, "a3", NOV).await, "A-003");
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn failed_create_does_not_consume_number(pool: PgPool) {
    let ctx = setup(&pool, Some("A-{000}")).await;
    assert_eq!(create_no(&pool, &ctx, "dup", NOV).await, "A-001");
    // 違反「不可重複」：取號後寫入失敗，號碼隨 rollback 還原
    let err = create_at(&pool, &ctx, "dup", NOV).await.unwrap_err();
    assert!(matches!(err, CoreError::InvalidFields(_)), "{err:?}");
    assert_eq!(create_no(&pool, &ctx, "ok", NOV).await, "A-002");
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn generated_number_colliding_with_existing_is_rejected(pool: PgPool) {
    let ctx = setup(&pool, Some("{yy}-{000}")).await;
    assert_eq!(create_no(&pool, &ctx, "a", NOV).await, "26-001");

    // 新規則沒有日期代碼，期間不同而重新從 1 開始，產生的 26-001 與既有單號重複
    set_pattern(&pool, &ctx, Some("26-{000}")).await;
    let err = create_at(&pool, &ctx, "b", NOV).await.unwrap_err();
    match err {
        CoreError::Validation(message) => assert_eq!(message, RECORD_NO_TAKEN),
        other => panic!("預期 Validation，實際為 {other:?}"),
    }
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn entities_have_independent_sequences(pool: PgPool) {
    let ctx = setup(&pool, Some("RO-{000}")).await;
    schema::create_entity(
        &pool,
        &ctx,
        &CreateEntityRequest {
            key: "other".into(),
            label: "其他".into(),
            icon: None,
            kind: EntityKind::Master,
            numbering_pattern: Some("RO-{000}".into()),
            title_template: None,
            sort_order: 0,
        },
        None,
        NOV,
    )
    .await
    .unwrap();
    let field: CreateFieldRequest = serde_json::from_value(json!({
        "field_key": "name", "label": "名稱", "data_type": "text",
    }))
    .unwrap();
    schema::create_field(&pool, &ctx, "other", &field, None, NOV)
        .await
        .unwrap();

    let mut numbers = HashSet::new();
    numbers.insert(create_no(&pool, &ctx, "a", NOV).await);
    let other = records::create_record(
        &pool,
        &ctx,
        "other",
        &CreateRecordRequest::default(),
        None,
        NOV,
    )
    .await
    .unwrap();
    // 不同資料類別各自從 1 開始，單號可以相同
    assert_eq!(other.record_no.as_deref(), Some("RO-001"));
    assert!(numbers.contains("RO-001"));

    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_data FROM audit_logs WHERE entity_key = 'other' AND record_id = $1",
    )
    .bind(other.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    // 操作紀錄包含產生的單號
    assert_eq!(audit["record_no"], "RO-001");
}
