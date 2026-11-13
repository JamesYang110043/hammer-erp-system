//! 0003_customization_tables 的整合測試：唯一約束、CHECK 約束、索引。
//!
//! 執行方式同 `foundation.rs`，需要設定 `DATABASE_URL`。

#![allow(clippy::unwrap_used)]

use hammer_db::sqlx::{self, PgPool};
use uuid::Uuid;

/// §7.3 列出的全部欄位資料型態。
const DATA_TYPES: &[&str] = &[
    "text",
    "long_text",
    "integer",
    "decimal",
    "money",
    "date",
    "datetime",
    "boolean",
    "select",
    "multi_select",
    "relation",
    "file",
    "formula",
];

/// 建立一個 `custom_records` 儲存的 master 資料類別，回傳 id。
async fn insert_entity(pool: &PgPool, key: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entity_definitions (id, key, label, kind, storage)
         VALUES ($1, $2, $2, 'master', 'custom_records')",
    )
    .bind(id)
    .bind(key)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_field(
    pool: &PgPool,
    entity_id: Uuid,
    field_key: &str,
    data_type: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO field_definitions (id, entity_id, field_key, label, data_type, storage)
         VALUES ($1, $2, $3, $3, $4, 'json')",
    )
    .bind(Uuid::now_v7())
    .bind(entity_id)
    .bind(field_key)
    .bind(data_type)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 取出資料庫錯誤的 SQLSTATE 與約束名稱。
fn db_error(err: &sqlx::Error) -> (String, Option<String>) {
    let db = err.as_database_error().expect("應為資料庫錯誤");
    (
        db.code().unwrap().into_owned(),
        db.constraint().map(str::to_owned),
    )
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn duplicate_field_key_in_same_entity_violates_unique(pool: PgPool) {
    let entity = insert_entity(&pool, "repair_order").await;
    insert_field(&pool, entity, "note", "text").await.unwrap();

    let err = insert_field(&pool, entity, "note", "long_text")
        .await
        .unwrap_err();
    assert_eq!(
        db_error(&err),
        (
            "23505".to_owned(),
            Some("field_definitions_entity_field_key_key".to_owned())
        )
    );
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn same_field_key_in_different_entities_is_allowed(pool: PgPool) {
    let a = insert_entity(&pool, "repair_order").await;
    let b = insert_entity(&pool, "visit_log").await;
    insert_field(&pool, a, "note", "text").await.unwrap();
    insert_field(&pool, b, "note", "text").await.unwrap();
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn duplicate_entity_key_violates_unique(pool: PgPool) {
    insert_entity(&pool, "repair_order").await;
    let err = sqlx::query(
        "INSERT INTO entity_definitions (id, key, label, kind, storage)
         VALUES ($1, 'repair_order', 'x', 'master', 'custom_records')",
    )
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(db_error(&err).0, "23505");
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn all_spec_data_types_are_accepted(pool: PgPool) {
    let entity = insert_entity(&pool, "repair_order").await;
    for data_type in DATA_TYPES {
        insert_field(&pool, entity, &format!("f_{data_type}"), data_type)
            .await
            .unwrap();
    }
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn unknown_data_type_violates_check(pool: PgPool) {
    let entity = insert_entity(&pool, "repair_order").await;
    for data_type in ["float", "string", "TEXT", ""] {
        let err = insert_field(&pool, entity, "f", data_type)
            .await
            .unwrap_err();
        assert_eq!(
            db_error(&err),
            (
                "23514".to_owned(),
                Some("field_definitions_data_type_check".to_owned())
            ),
            "data_type = {data_type:?}"
        );
    }
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn enum_columns_reject_unknown_values(pool: PgPool) {
    let cases = [
        "INSERT INTO entity_definitions (id, key, label, kind, storage)
         VALUES ($1, 'a', 'a', 'report', 'custom_records')",
        "INSERT INTO entity_definitions (id, key, label, kind, storage)
         VALUES ($1, 'a', 'a', 'master', 'jsonb')",
        "INSERT INTO schema_change_logs (id, target_type, target_id, action)
         VALUES ($1, 'view', $1, 'create')",
        "INSERT INTO schema_change_logs (id, target_type, target_id, action)
         VALUES ($1, 'field', $1, 'delete')",
    ];
    for sql in cases {
        let err = sqlx::query(sql)
            .bind(Uuid::now_v7())
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(db_error(&err).0, "23514", "{sql}");
    }
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn custom_record_belongs_to_entity(pool: PgPool) {
    let entity = insert_entity(&pool, "repair_order").await;
    sqlx::query(
        "INSERT INTO custom_records (id, entity_id, data)
         VALUES ($1, $2, '{\"note\":\"a\",\"amount\":\"12.3400\"}')",
    )
    .bind(Uuid::now_v7())
    .bind(entity)
    .execute(&pool)
    .await
    .unwrap();

    // data 必須是 JSON 物件
    let err = sqlx::query("INSERT INTO custom_records (id, entity_id, data) VALUES ($1, $2, '[]')")
        .bind(Uuid::now_v7())
        .bind(entity)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(db_error(&err).0, "23514");
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn number_sequence_is_unique_per_entity_and_period(pool: PgPool) {
    let entity = insert_entity(&pool, "repair_order").await;
    let insert =
        "INSERT INTO number_sequences (id, entity_id, period_key) VALUES ($1, $2, '202611')";
    sqlx::query(insert)
        .bind(Uuid::now_v7())
        .bind(entity)
        .execute(&pool)
        .await
        .unwrap();
    let err = sqlx::query(insert)
        .bind(Uuid::now_v7())
        .bind(entity)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(db_error(&err).0, "23505");
}

#[sqlx::test(migrator = "hammer_db::MIGRATOR")]
async fn custom_records_has_required_indexes(pool: PgPool) {
    let defs: Vec<String> =
        sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE tablename = 'custom_records'")
            .fetch_all(&pool)
            .await
            .unwrap();
    for expected in [
        "btree (entity_id)",
        "btree (parent_record_id)",
        "gin (data)",
    ] {
        assert!(
            defs.iter().any(|d| d.ends_with(expected)),
            "缺少索引 {expected}：{defs:?}"
        );
    }
}
