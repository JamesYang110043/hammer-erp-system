//! `hammer_core::auth` 的資料庫整合測試：session 的建立、查詢、更新、撤銷，以及讀取安全設定。
//!
//! 使用 `#[sqlx::test]`，執行前需設定 `DATABASE_URL`，見 README「整合測試」。

#![allow(clippy::unwrap_used)]

use hammer_core::auth::{SecuritySettings, SessionStatus, hash_token, session, session_status};
use hammer_db::sqlx::{self, PgPool};
use hammer_shared::{Id, new_id, role};
use time::{Duration, OffsetDateTime};

/// 建立一個測試帳號，回傳 id。
async fn create_user(pool: &PgPool, username: &str) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, role_id)
         SELECT $1, $2, $2, 'x', id FROM roles WHERE code = $3",
    )
    .bind(id)
    .bind(username)
    .bind(role::EDITOR)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// 資料庫的 TIMESTAMPTZ 精度是微秒，測試用的時間取整到秒，比較時才不會有誤差。
fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc().replace_nanosecond(0).unwrap()
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn create_and_find_session(pool: PgPool) {
    let user_id = create_user(&pool, "alice").await;
    let settings = SecuritySettings::default();
    let t0 = now();

    let (token, row) = session::create(&pool, user_id, Some("MacBook / 0.1.0"), t0, &settings)
        .await
        .unwrap();

    assert_eq!(row.user_id, user_id);
    assert_eq!(row.id.get_version_num(), 7);
    assert_eq!(row.client_info.as_deref(), Some("MacBook / 0.1.0"));
    assert_eq!(row.created_at, t0);
    assert_eq!(row.last_seen_at, t0);
    assert_eq!(row.expires_at, t0 + Duration::minutes(480));
    assert_eq!(row.revoked_at, None);

    // 資料庫只存雜湊，不存 token 本身
    assert_eq!(row.token_hash, hash_token(token.as_str()));
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(stored, vec![token.hash()]);
    assert_ne!(stored[0], token.as_str());

    let found = session::find_by_token(&pool, token.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, row);
    assert_eq!(session_status(&found, t0, &settings), SessionStatus::Active);

    assert!(
        session::find_by_token(&pool, "unknown-token")
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn touch_extends_idle_window(pool: PgPool) {
    let user_id = create_user(&pool, "alice").await;
    let settings = SecuritySettings::default();
    let t0 = now();
    let (token, row) = session::create(&pool, user_id, None, t0, &settings)
        .await
        .unwrap();

    let t1 = t0 + Duration::hours(7);
    assert!(session::touch(&pool, row.id, t1, &settings).await.unwrap());

    let found = session::find_by_token(&pool, token.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.last_seen_at, t1);
    assert_eq!(found.expires_at, t1 + Duration::minutes(480));
    assert_eq!(found.created_at, t0);
    // 沒有 touch 的話 t0 + 8 小時已閒置逾時；touch 後仍有效
    assert!(session_status(&found, t0 + Duration::hours(10), &settings).is_active());
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn revoke_single_session(pool: PgPool) {
    let user_id = create_user(&pool, "alice").await;
    let settings = SecuritySettings::default();
    let t0 = now();
    let (token_a, a) = session::create(&pool, user_id, None, t0, &settings)
        .await
        .unwrap();
    let (token_b, _) = session::create(&pool, user_id, None, t0, &settings)
        .await
        .unwrap();

    let t1 = t0 + Duration::minutes(1);
    assert!(session::revoke(&pool, a.id, t1).await.unwrap());
    // 重複撤銷不改變撤銷時間
    assert!(
        !session::revoke(&pool, a.id, t1 + Duration::minutes(1))
            .await
            .unwrap()
    );

    let a = session::find_by_token(&pool, token_a.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.revoked_at, Some(t1));
    assert_eq!(session_status(&a, t1, &settings), SessionStatus::Revoked);

    // 已撤銷的 session 不會被 touch
    assert!(!session::touch(&pool, a.id, t1, &settings).await.unwrap());

    // 同一使用者的其他 session 不受影響
    let b = session::find_by_token(&pool, token_b.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.revoked_at, None);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn revoke_all_sessions_of_user(pool: PgPool) {
    let alice = create_user(&pool, "alice").await;
    let bob = create_user(&pool, "bob").await;
    let settings = SecuritySettings::default();
    let t0 = now();
    let (_, a1) = session::create(&pool, alice, None, t0, &settings)
        .await
        .unwrap();
    session::create(&pool, alice, None, t0, &settings)
        .await
        .unwrap();
    session::create(&pool, alice, None, t0, &settings)
        .await
        .unwrap();
    let (token_bob, _) = session::create(&pool, bob, None, t0, &settings)
        .await
        .unwrap();

    // 先登出其中一個，全部撤銷時只計算尚未撤銷的
    session::revoke(&pool, a1.id, t0).await.unwrap();
    let t1 = t0 + Duration::minutes(5);
    assert_eq!(
        session::revoke_all_for_user(&pool, alice, t1)
            .await
            .unwrap(),
        2
    );

    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(alice)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 0);

    // 已登出的那筆保留原本的撤銷時間
    let a1_revoked: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT revoked_at FROM sessions WHERE id = $1")
            .bind(a1.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(a1_revoked, Some(t0));

    // 其他使用者不受影響
    let bob_session = session::find_by_token(&pool, token_bob.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bob_session.revoked_at, None);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn security_settings_load_migration_defaults(pool: PgPool) {
    let settings = SecuritySettings::load(&pool).await.unwrap();
    assert_eq!(settings, SecuritySettings::default());
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn security_settings_load_overrides_and_missing_keys(pool: PgPool) {
    sqlx::query("UPDATE settings SET value = '3' WHERE key = 'security.max_failed_logins'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM settings WHERE key = 'security.session_idle_minutes'")
        .execute(&pool)
        .await
        .unwrap();

    let settings = SecuritySettings::load(&pool).await.unwrap();
    assert_eq!(settings.max_failed_logins, 3);
    assert_eq!(settings.lockout_minutes, 15);
    // 資料庫沒有值時使用預設值
    assert_eq!(settings.session_idle_minutes, 480);
}
