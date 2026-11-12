//! `hammer_core::audit` 的資料庫整合測試：操作紀錄與業務資料在同一個 transaction 內寫入。
//!
//! 使用 `#[sqlx::test]`，執行前需設定 `DATABASE_URL`，見 README「整合測試」。

#![allow(clippy::unwrap_used)]

use hammer_core::audit::{self, AuditDiff, AuditEntry};
use hammer_db::sqlx::{self, PgPool};
use hammer_shared::{Id, audit as action, new_id, role};
use serde_json::{Value, json};
use time::OffsetDateTime;

/// 在 transaction 內新增一個帳號並寫入 `create` 紀錄，回傳帳號 id。
async fn create_user_with_audit(tx: &mut sqlx::PgTransaction<'_>, username: &str) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, role_id)
         SELECT $1, $2, $2, 'secret-hash', id FROM roles WHERE code = $3",
    )
    .bind(id)
    .bind(username)
    .bind(role::EDITOR)
    .execute(&mut **tx)
    .await
    .unwrap();

    let after = json!({ "username": username, "password_hash": "secret-hash" });
    audit::record(
        tx,
        &AuditEntry {
            user_id: None,
            action: action::CREATE,
            entity_key: Some("user"),
            record_id: Some(id),
            diff: AuditDiff::between(None, Some(&after)).unwrap(),
            client_info: Some("MacBook / 0.1.0"),
            occurred_at: OffsetDateTime::now_utc(),
        },
    )
    .await
    .unwrap();
    id
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn rollback_discards_audit_log(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    create_user_with_audit(&mut tx, "alice").await;
    // transaction 內看得到紀錄
    let inside: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(inside, 1);
    tx.rollback().await.unwrap();

    assert_eq!(count(&pool, "SELECT count(*) FROM users").await, 0);
    assert_eq!(count(&pool, "SELECT count(*) FROM audit_logs").await, 0);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn commit_keeps_audit_log(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    let user_id = create_user_with_audit(&mut tx, "alice").await;
    tx.commit().await.unwrap();

    #[derive(sqlx::FromRow)]
    struct Row {
        action: String,
        entity_key: Option<String>,
        record_id: Option<Id>,
        before_data: Option<Value>,
        after_data: Option<Value>,
        client_info: Option<String>,
    }

    let row: Row = sqlx::query_as(
        "SELECT action, entity_key, record_id, before_data, after_data, client_info FROM audit_logs",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.action, action::CREATE);
    assert_eq!(row.entity_key.as_deref(), Some("user"));
    assert_eq!(row.record_id, Some(user_id));
    assert_eq!(row.before_data, None);
    assert_eq!(row.after_data, Some(json!({ "username": "alice" })));
    assert_eq!(row.client_info.as_deref(), Some("MacBook / 0.1.0"));
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn unknown_action_is_rejected(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    let result = audit::record(
        &mut tx,
        &AuditEntry {
            user_id: None,
            action: "rename",
            entity_key: None,
            record_id: None,
            diff: AuditDiff::default(),
            client_info: None,
            occurred_at: OffsetDateTime::now_utc(),
        },
    )
    .await;
    assert!(result.is_err());
    tx.commit().await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM audit_logs").await, 0);
}
