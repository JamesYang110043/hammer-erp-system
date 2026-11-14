//! 附件（`file` 欄位）的 API 整合測試（#71）。
//!
//! - 上傳 `POST /api/v1/attachments`：sha256、大小上限（413）、權限（403）
//! - 綁定：寫入 `file` 欄位時綁定；同一個附件不可綁到第二筆資料或第二個欄位（422）
//! - 替換、清空：舊附件標記刪除、檔案保留；資料軟刪除時附件保留
//! - 下載 `GET /api/v1/attachments/{id}`：內容一致、中文檔名
//! - 清理：只刪除超過時限且未綁定的附件
//!
//! 純邏輯（變動判斷、檢查、檔名、MIME）在 `hammer_core::attachments`、`hammer_shared::attachment`
//! 的單元測試。使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use hammer_core::attachments::{AttachmentStore, cleanup_unbound};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::attachment::Attachment;
use hammer_shared::record::Record;
use hammer_shared::{ApiError, ErrorCode, Id, new_id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

/// 有附件欄位的資料類別（維修單）：`note`（text）、`photo`、`doc`（file）。
const REPAIR: &str = "repair_order";
/// 測試用的單檔大小上限。
const MAX_BYTES: u64 = 64;
const BOUNDARY: &str = "hammer-test-boundary";

/// multipart 的一個欄位：`(欄位名稱, 檔名, Content-Type, 內容)`。
type Part<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a [u8]);

/// 每個測試使用自己的附件目錄，結束時刪除。
struct Env {
    pool: PgPool,
    dir: PathBuf,
}

impl Env {
    fn new(pool: &PgPool) -> Self {
        let dir = std::env::temp_dir().join(format!("hammer-attachments-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            pool: pool.clone(),
            dir,
        }
    }

    fn app(&self) -> Router {
        let dir = self.dir.to_string_lossy().into_owned();
        let config = Config::from_sources(None, |name| match name {
            "DATABASE_URL" => Some("postgres://unused".to_owned()),
            "HAMMER_ATTACHMENTS_DIR" => Some(dir.clone()),
            "HAMMER_ATTACHMENT_MAX_BYTES" => Some(MAX_BYTES.to_string()),
            _ => None,
        })
        .unwrap();
        router(AppState::new(self.pool.clone(), config))
    }

    fn store(&self) -> AttachmentStore {
        AttachmentStore::new(&self.dir, MAX_BYTES)
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
        let res = self.app().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, headers, bytes)
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = request(method, uri, token);
        let body = match body {
            Some(json) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let (status, _, bytes) = self.send(req.body(body).unwrap()).await;
        (status, to_json(&bytes))
    }

    /// 以 multipart 上傳 `parts`：`(欄位名稱, 檔名, Content-Type, 內容)`。
    async fn upload_parts(&self, token: &str, parts: &[Part<'_>]) -> (StatusCode, Value) {
        let req = request(Method::POST, "/api/v1/attachments", token)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(multipart(parts)))
            .unwrap();
        let (status, _, bytes) = self.send(req).await;
        (status, to_json(&bytes))
    }

    async fn upload_raw(&self, token: &str, name: &str, content: &[u8]) -> (StatusCode, Value) {
        self.upload_parts(token, &[("file", Some(name), Some("text/plain"), content)])
            .await
    }

    async fn upload(&self, token: &str, name: &str, content: &[u8]) -> Attachment {
        let (status, body) = self.upload_raw(token, name, content).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        serde_json::from_value(body).unwrap()
    }

    async fn download(&self, token: &str, id: Id) -> (StatusCode, HeaderMap, Bytes) {
        let req = request(Method::GET, &format!("/api/v1/attachments/{id}"), token)
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }

    async fn create_raw(&self, token: &str, data: Value) -> (StatusCode, Value) {
        self.call(
            Method::POST,
            &format!("/api/v1/entities/{REPAIR}/records"),
            token,
            Some(json!({ "data": data })),
        )
        .await
    }

    async fn create(&self, token: &str, data: Value) -> Record {
        let (status, body) = self.create_raw(token, data).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        serde_json::from_value(body).unwrap()
    }

    async fn update_raw(&self, token: &str, record: &Record, data: Value) -> (StatusCode, Value) {
        self.call(
            Method::PATCH,
            &format!("/api/v1/entities/{REPAIR}/records/{}", record.id),
            token,
            Some(json!({ "data": data, "version": record.version })),
        )
        .await
    }

    async fn update(&self, token: &str, record: &Record, data: Value) -> Record {
        let (status, body) = self.update_raw(token, record, data).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value(body).unwrap()
    }

    /// 附件目錄中（不含暫存目錄）的檔案名稱。
    fn stored_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.dir)
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.file_type().unwrap().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn temp_files(&self) -> usize {
        match std::fs::read_dir(self.dir.join("tmp")) {
            Ok(entries) => entries.count(),
            Err(_) => 0,
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn request(method: Method, uri: &str, token: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
}

fn to_json(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap()
    }
}

fn multipart(parts: &[Part<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, file_name, content_type, content) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let mut disposition = format!("Content-Disposition: form-data; name=\"{name}\"");
        if let Some(file_name) = file_name {
            disposition.push_str(&format!("; filename=\"{file_name}\""));
        }
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(b"\r\n");
        if let Some(ct) = content_type {
            body.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn sha256_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn assert_error(status: StatusCode, body: &Value, expected: ErrorCode) -> ApiError {
    assert_eq!(status.as_u16(), expected.http_status(), "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, expected, "{body}");
    assert!(!err.message.is_empty());
    err
}

/// 422 VALIDATION，且 `details.fields` 的 `field_key` 依序為 `keys`。
fn assert_invalid_fields(status: StatusCode, body: &Value, keys: &[&str]) {
    let err = assert_error(status, body, ErrorCode::Validation);
    let fields = err.details.unwrap()["fields"].as_array().unwrap().clone();
    let actual: Vec<_> = fields
        .iter()
        .map(|f| f["field_key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(actual, keys, "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

/// 建立 [`REPAIR`]，回傳管理者 token。
async fn setup(env: &Env) -> String {
    let admin = token_for(&env.pool, "admin", role::ADMIN).await;
    let (status, body) = env
        .call(
            Method::POST,
            "/api/v1/entities",
            &admin,
            Some(json!({ "key": REPAIR, "label": "維修單", "kind": "master" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    for (key, data_type) in [("note", "text"), ("photo", "file"), ("doc", "file")] {
        let (status, body) = env
            .call(
                Method::POST,
                &format!("/api/v1/entities/{REPAIR}/fields"),
                &admin,
                Some(json!({ "field_key": key, "label": key, "data_type": data_type })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    admin
}

/// 附件資料列的綁定位置與是否已標記刪除：`(entity_key, record_id, field_key, deleted)`。
async fn binding(pool: &PgPool, id: Id) -> (Option<String>, Option<Id>, Option<String>, bool) {
    sqlx::query_as(
        "SELECT entity_key, record_id, field_key, deleted_at IS NOT NULL \
         FROM attachments WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// 附件的操作紀錄動作，依時間順序。
async fn audit_actions(pool: &PgPool, id: Id) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT action FROM audit_logs WHERE entity_key = 'attachments' AND record_id = $1 \
         ORDER BY occurred_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn exists(dir: &Path, id: Id) -> bool {
    dir.join(id.to_string()).is_file()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_bind_and_download(pool: PgPool) {
    let env = Env::new(&pool);
    let admin = setup(&env).await;
    let content = "維修照片內容 hello".as_bytes();

    let attachment = env.upload(&admin, "維修照片 1.jpg", content).await;
    assert_eq!(attachment.file_name, "維修照片 1.jpg");
    assert_eq!(attachment.mime_type, "text/plain");
    assert_eq!(attachment.size_bytes, content.len() as u64);
    assert_eq!(attachment.sha256, sha256_hex(content));
    // 磁碟上以 id 命名，不使用原始檔名
    assert_eq!(env.stored_files(), [attachment.id.to_string()]);
    assert_eq!(env.temp_files(), 0);
    assert_eq!(
        binding(&pool, attachment.id).await,
        (None, None, None, false)
    );

    // 綁定
    let record = env
        .create(&admin, json!({ "note": "a", "photo": attachment.id }))
        .await;
    assert_eq!(record.data["photo"], json!(attachment.id));
    assert_eq!(
        binding(&pool, attachment.id).await,
        (
            Some(REPAIR.to_owned()),
            Some(record.id),
            Some("photo".to_owned()),
            false
        )
    );
    assert_eq!(
        audit_actions(&pool, attachment.id).await,
        ["create", "update"]
    );

    // 下載：內容一致，中文檔名，唯讀者也可以下載
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let (status, headers, bytes) = env.download(&viewer, attachment.id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], content);
    assert_eq!(sha256_hex(&bytes), attachment.sha256);
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(
        disposition.contains("filename*=UTF-8''%E7%B6%AD%E4%BF%AE%E7%85%A7%E7%89%87%201.jpg"),
        "{disposition}"
    );

    // 篩選：file 欄位只支援 empty
    let (status, body) = env
        .call(
            Method::GET,
            &format!("/api/v1/entities/{REPAIR}/records?filter.photo.empty=false"),
            &admin,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], json!(1));
    let (status, body) = env
        .call(
            Method::GET,
            &format!(
                "/api/v1/entities/{REPAIR}/records?filter.photo.eq={}",
                attachment.id
            ),
            &admin,
            None,
        )
        .await;
    assert_error(status, &body, ErrorCode::Validation);

    // 資料被軟刪除：附件保留，仍可下載
    let (status, body) = env
        .call(
            Method::DELETE,
            &format!(
                "/api/v1/entities/{REPAIR}/records/{}?version={}",
                record.id, record.version
            ),
            &admin,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!binding(&pool, attachment.id).await.3);
    let (status, _, bytes) = env.download(&admin, attachment.id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], content);

    // 不存在的附件
    let (status, _, bytes) = env.download(&admin, new_id()).await;
    assert_error(status, &to_json(&bytes), ErrorCode::NotFound);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_limits_and_permissions(pool: PgPool) {
    let env = Env::new(&pool);
    let admin = setup(&env).await;

    // 剛好等於上限可以上傳；超過 → 413，不留下檔案與資料
    let exact = vec![b'a'; MAX_BYTES as usize];
    env.upload(&admin, "exact.txt", &exact).await;
    let over = vec![b'a'; MAX_BYTES as usize + 1];
    let (status, body) = env.upload_raw(&admin, "over.txt", &over).await;
    assert_error(status, &body, ErrorCode::PayloadTooLarge);
    // request 本身超過上傳路由的 body 上限
    let huge = vec![b'a'; 200 * 1024];
    let (status, body) = env.upload_raw(&admin, "huge.txt", &huge).await;
    assert_error(status, &body, ErrorCode::PayloadTooLarge);
    // 檔案之前的其他欄位很大：由 body 上限擋下
    let (status, body) = env
        .upload_parts(
            &admin,
            &[
                ("note", None, None, &huge),
                ("file", Some("a.txt"), None, b"abc"),
            ],
        )
        .await;
    assert_error(status, &body, ErrorCode::PayloadTooLarge);
    assert_eq!(env.stored_files().len(), 1);
    assert_eq!(env.temp_files(), 0);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // 唯讀者不能上傳
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let (status, body) = env.upload_raw(&viewer, "a.txt", b"abc").await;
    assert_error(status, &body, ErrorCode::Forbidden);
    // 未登入
    let (status, body) = env.upload_raw("invalid-token", "a.txt", b"abc").await;
    assert_error(status, &body, ErrorCode::Unauthorized);

    // 沒有 file 欄位、沒有檔名 → 422；檔名去除路徑
    let (status, body) = env
        .upload_parts(&admin, &[("other", Some("a.txt"), None, b"abc")])
        .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["field"], json!("file"));
    let (status, body) = env
        .upload_parts(&admin, &[("file", None, None, b"abc")])
        .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["field"], json!("file"));
    let (status, body) = env
        .upload_parts(
            &admin,
            &[
                ("note", None, None, b"ignored"),
                ("file", Some("../../etc/passwd"), None, b"abc"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["file_name"], json!("passwd"));
    assert_eq!(body["mime_type"], json!("application/octet-stream"));
    assert_eq!(env.temp_files(), 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn attachment_binds_only_once(pool: PgPool) {
    let env = Env::new(&pool);
    let admin = setup(&env).await;
    let a = env.upload(&admin, "a.txt", b"a").await;
    let b = env.upload(&admin, "b.txt", b"b").await;

    let first = env.create(&admin, json!({ "photo": a.id })).await;

    // 同一個附件綁到第二筆資料 → 422
    let (status, body) = env.create_raw(&admin, json!({ "photo": a.id })).await;
    assert_invalid_fields(status, &body, &["photo"]);
    // 綁到同一筆資料的另一個欄位 → 422
    let (status, body) = env.update_raw(&admin, &first, json!({ "doc": a.id })).await;
    assert_invalid_fields(status, &body, &["doc"]);
    // 同一個 request 用在兩個欄位 → 422
    let (status, body) = env
        .create_raw(&admin, json!({ "photo": b.id, "doc": b.id }))
        .await;
    assert_invalid_fields(status, &body, &["doc"]);
    // 不存在的附件、格式錯誤 → 422
    let (status, body) = env.create_raw(&admin, json!({ "photo": new_id() })).await;
    assert_invalid_fields(status, &body, &["photo"]);
    let (status, body) = env.create_raw(&admin, json!({ "doc": "x.pdf" })).await;
    assert_invalid_fields(status, &body, &["doc"]);
    // 失敗的寫入不會綁定
    assert_eq!(binding(&pool, b.id).await, (None, None, None, false));

    // 修改其他欄位：沒變動的附件不重新檢查
    let first = env.update(&admin, &first, json!({ "note": "x" })).await;
    assert_eq!(audit_actions(&pool, a.id).await, ["create", "update"]);

    // 換成新附件：舊附件標記刪除（檔案保留、不能下載），新附件綁定
    let first = env.update(&admin, &first, json!({ "photo": b.id })).await;
    assert!(binding(&pool, a.id).await.3);
    assert!(exists(&env.dir, a.id));
    let (status, _, bytes) = env.download(&admin, a.id).await;
    assert_error(status, &to_json(&bytes), ErrorCode::NotFound);
    assert_eq!(
        audit_actions(&pool, a.id).await,
        ["create", "update", "delete"]
    );
    assert_eq!(binding(&pool, b.id).await.2.as_deref(), Some("photo"));
    // 已標記刪除的附件不能再使用
    let (status, body) = env.update_raw(&admin, &first, json!({ "doc": a.id })).await;
    assert_invalid_fields(status, &body, &["doc"]);

    // 清空：舊附件標記刪除
    env.update(&admin, &first, json!({ "photo": null })).await;
    assert!(binding(&pool, b.id).await.3);
    assert!(exists(&env.dir, b.id));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn file_field_rules(pool: PgPool) {
    let env = Env::new(&pool);
    let admin = setup(&env).await;

    // 不可設定預設值
    let (status, body) = env
        .call(
            Method::POST,
            &format!("/api/v1/entities/{REPAIR}/fields"),
            &admin,
            Some(json!({
                "field_key": "extra", "label": "extra", "data_type": "file",
                "default_value": new_id(),
            })),
        )
        .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["field"], json!("default_value"));

    // 不能變更型態
    let (status, body) = env
        .call(
            Method::GET,
            &format!("/api/v1/entities/{REPAIR}"),
            &admin,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let photo = body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["field_key"] == "photo")
        .unwrap()
        .clone();
    let (status, body) = env
        .call(
            Method::POST,
            &format!("/api/v1/entities/{REPAIR}/fields/photo/change-type"),
            &admin,
            Some(json!({ "data_type": "text", "version": photo["version"] })),
        )
        .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["field"], json!("data_type"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cleanup_removes_only_old_unbound(pool: PgPool) {
    let env = Env::new(&pool);
    let admin = setup(&env).await;
    let old_unbound = env.upload(&admin, "old.txt", b"old").await;
    let new_unbound = env.upload(&admin, "new.txt", b"new").await;
    let old_bound = env.upload(&admin, "bound.txt", b"bound").await;
    let old_replaced = env.upload(&admin, "replaced.txt", b"replaced").await;
    let record = env
        .create(
            &admin,
            json!({ "photo": old_bound.id, "doc": old_replaced.id }),
        )
        .await;
    env.update(&admin, &record, json!({ "doc": null })).await;

    // 除了 new_unbound，都改成 25 小時前上傳
    sqlx::query("UPDATE attachments SET created_at = now() - interval '25 hours' WHERE id <> $1")
        .bind(new_unbound.id)
        .execute(&pool)
        .await
        .unwrap();
    // 程序中止時留下的暫存檔
    std::fs::create_dir_all(env.dir.join("tmp")).unwrap();
    std::fs::write(env.dir.join("tmp").join("stale"), b"x").unwrap();

    let now = OffsetDateTime::now_utc();
    let report = cleanup_unbound(&pool, &env.store(), now).await.unwrap();
    assert_eq!(report.attachments, 1);
    assert_eq!(report.temp_files, 0);

    let ids: Vec<Id> = sqlx::query_scalar("SELECT id FROM attachments ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    let mut expected = vec![new_unbound.id, old_bound.id, old_replaced.id];
    expected.sort();
    assert_eq!(ids, expected);
    assert!(!exists(&env.dir, old_unbound.id));
    for kept in [new_unbound.id, old_bound.id, old_replaced.id] {
        assert!(exists(&env.dir, kept));
    }
    assert_eq!(
        audit_actions(&pool, old_unbound.id).await,
        ["create", "delete"]
    );
    let actor: Option<Id> = sqlx::query_scalar(
        "SELECT user_id FROM audit_logs WHERE record_id = $1 AND action = 'delete'",
    )
    .bind(old_unbound.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor, None);

    // 25 小時後：剛上傳的也超過時限；暫存檔也一併清除
    let later = now + Duration::hours(25);
    let report = cleanup_unbound(&pool, &env.store(), later).await.unwrap();
    assert_eq!(report.attachments, 1);
    assert_eq!(report.temp_files, 1);
    assert!(!exists(&env.dir, new_unbound.id));
    assert!(exists(&env.dir, old_bound.id));
}
