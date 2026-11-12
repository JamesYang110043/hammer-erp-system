//! 0001_foundation 與資料存取基礎設施的整合測試。
//!
//! 使用 `#[sqlx::test]`：每個測試建立獨立的暫時資料庫，結束後自動刪除。
//! 執行前需設定 `DATABASE_URL`（使用者需有 CREATEDB 權限），見 README。

// clippy.toml 的 allow-unwrap-in-tests 不涵蓋測試檔中的輔助函式
#![allow(clippy::unwrap_used)]

use hammer_db::sqlx::{self, PgPool};
use hammer_db::{DbError, ensure_version_matched, run_migrations};
use time::OffsetDateTime;

/// 建立一張含共通欄位的測試資料表，並掛上 `set_updated_at` trigger。
async fn create_sample_table(pool: &PgPool) {
    sqlx::query(
        r#"
        CREATE TABLE samples (
            id          UUID PRIMARY KEY,
            name        TEXT NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            deleted_at  TIMESTAMPTZ,
            version     INTEGER NOT NULL DEFAULT 1
        )
        "#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TRIGGER trg_samples_set_updated_at
             BEFORE UPDATE ON samples
             FOR EACH ROW EXECUTE FUNCTION set_updated_at()",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_sample(pool: &PgPool, updated_at: Option<OffsetDateTime>) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO samples (id, name, updated_at) VALUES ($1, 'a', COALESCE($2, now()))")
        .bind(id)
        .bind(updated_at)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn update_name(
    pool: &PgPool,
    id: uuid::Uuid,
    name: &str,
    version: i32,
) -> Result<(), DbError> {
    let result = sqlx::query(
        "UPDATE samples SET name = $3, version = version + 1 WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .bind(name)
    .execute(pool)
    .await?;
    ensure_version_matched(result)?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn run_migrations_on_empty_database_is_idempotent(pool: PgPool) {
    run_migrations(&pool).await.unwrap();
    run_migrations(&pool).await.unwrap();

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, hammer_db::MIGRATOR.iter().count() as i64);

    let has_fn: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_proc WHERE proname = 'set_updated_at')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(has_fn);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn set_updated_at_trigger_updates_timestamp(pool: PgPool) {
    create_sample_table(&pool).await;

    let old = time::macros::datetime!(2000-01-01 0:00 UTC);
    let id = insert_sample(&pool, Some(old)).await;

    sqlx::query("UPDATE samples SET name = 'b' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let (updated_at, db_now): (OffsetDateTime, OffsetDateTime) =
        sqlx::query_as("SELECT updated_at, now() FROM samples WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        updated_at > old,
        "updated_at 應由 trigger 更新：{updated_at}"
    );
    assert!(updated_at <= db_now);
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn optimistic_lock_returns_conflict_on_version_mismatch(pool: PgPool) {
    create_sample_table(&pool).await;
    let id = insert_sample(&pool, None).await;

    // 版本相符：更新成功，version 遞增為 2
    update_name(&pool, id, "b", 1).await.unwrap();
    let version: i32 = sqlx::query_scalar("SELECT version FROM samples WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 2);

    // 以舊版本號更新：衝突，資料不變
    let err = update_name(&pool, id, "c", 1).await.unwrap_err();
    assert!(
        matches!(err, DbError::Conflict),
        "預期 Conflict，實際：{err:?}"
    );

    let (name, version): (String, i32) =
        sqlx::query_as("SELECT name, version FROM samples WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((name.as_str(), version), ("b", 2));
}
