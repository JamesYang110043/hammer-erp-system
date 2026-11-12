//! 0002_system_tables 的整合測試：角色權限對照、權限代碼一致性、users 的 updated_at trigger。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use hammer_db::sqlx::{self, PgPool};
use hammer_shared::{permission, role};
use time::OffsetDateTime;

async fn permissions_of(pool: &PgPool, role_code: &str) -> BTreeSet<String> {
    sqlx::query_scalar(
        "SELECT rp.permission
         FROM role_permissions rp JOIN roles r ON r.id = rp.role_id
         WHERE r.code = $1",
    )
    .bind(role_code)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

fn set(codes: &[&str]) -> BTreeSet<String> {
    codes.iter().map(|s| (*s).to_owned()).collect()
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn builtin_roles_match_shared_constants(pool: PgPool) {
    let rows: Vec<(String, bool)> = sqlx::query_as("SELECT code, is_system FROM roles")
        .fetch_all(&pool)
        .await
        .unwrap();
    let codes: BTreeSet<String> = rows.iter().map(|(c, _)| c.clone()).collect();
    assert_eq!(codes, set(role::ALL));
    assert!(
        rows.iter().all(|(_, is_system)| *is_system),
        "內建角色應為 is_system"
    );
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn role_permission_mapping_matches_spec(pool: PgPool) {
    let admin = permissions_of(&pool, role::ADMIN).await;
    assert_eq!(admin.len(), 8);
    assert_eq!(admin, set(permission::ALL));

    assert_eq!(
        permissions_of(&pool, role::EDITOR).await,
        set(&[
            permission::RECORD_READ,
            permission::RECORD_WRITE,
            permission::RECORD_POST,
            permission::DATA_EXPORT,
        ])
    );

    assert_eq!(
        permissions_of(&pool, role::VIEWER).await,
        set(&[permission::RECORD_READ, permission::DATA_EXPORT])
    );
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn permission_codes_in_db_equal_shared_all(pool: PgPool) {
    let codes: BTreeSet<String> =
        sqlx::query_scalar("SELECT DISTINCT permission FROM role_permissions")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .collect();
    assert_eq!(codes, set(permission::ALL));
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn security_settings_have_defaults(pool: PgPool) {
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'security.%' ORDER BY key")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![
            ("security.lockout_minutes".to_owned(), serde_json::json!(15)),
            (
                "security.max_failed_logins".to_owned(),
                serde_json::json!(5)
            ),
            (
                "security.session_idle_minutes".to_owned(),
                serde_json::json!(480)
            ),
        ]
    );
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn updating_user_bumps_updated_at(pool: PgPool) {
    let id = uuid::Uuid::now_v7();
    let old = time::macros::datetime!(2000-01-01 0:00 UTC);
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, role_id, updated_at)
         SELECT $1, 'alice', 'Alice', 'x', id, $2 FROM roles WHERE code = 'viewer'",
    )
    .bind(id)
    .bind(old)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("UPDATE users SET display_name = 'Alice B' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let updated_at: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        updated_at > old,
        "updated_at 應由 trigger 更新：{updated_at}"
    );
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn audit_log_allows_null_user(pool: PgPool) {
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, client_info)
         VALUES ($1, NULL, 'login_failed', 'test')",
    )
    .bind(uuid::Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
}
