//! 附件（`docs/architecture.md` §7.3 `file`、§8.4 `attachments`）。
//!
//! | 端點 | request | 成功回應 |
//! |---|---|---|
//! | `POST /api/v1/attachments` | `multipart/form-data`，檔案放在 [`UPLOAD_FIELD`] 欄位 | `201` [`Attachment`] |
//! | `GET /api/v1/attachments/{id}` | | `200` 檔案內容 |
//!
//! 上傳需要 `record.write`，下載需要 `record.read`。
//!
//! # 流程
//!
//! 1. 上傳檔案，取得附件 id。此時附件**未綁定**到任何資料
//! 2. 新增或修改資料時，把附件 id 寫入 `file` 欄位，伺服器把附件綁定到這筆資料的這個欄位
//! 3. 未綁定超過 [`UNBOUND_RETENTION_HOURS`] 小時的附件由伺服器清理（刪除資料與檔案）
//!
//! # 規則
//!
//! - 單檔大小上限由伺服器設定（`HAMMER_ATTACHMENT_MAX_BYTES`，預設 20 MiB），超過 → `413 PAYLOAD_TOO_LARGE`
//! - 檔名只存在資料庫；伺服器磁碟上以附件 id 命名。檔名去除路徑（`/`、`\` 之前的部分）與控制字元，
//!   最多 [`MAX_FILE_NAME_CHARS`] 個字元；處理後是空字串 → `422 VALIDATION`（`details.field = file`）
//! - MIME 使用 multipart 欄位的 `Content-Type`；沒有或格式不正確時為 `application/octet-stream`
//! - 寫入 `file` 欄位：值必須是未刪除、**未綁定**的附件 id；不存在、已被其他資料或欄位使用 →
//!   `422 VALIDATION`（`details.fields` 指出欄位）。修改資料時只檢查有變動的 `file` 欄位
//! - 欄位值被換掉或清空：舊附件標記為刪除（檔案保留），之後無法下載
//! - 資料被軟刪除：附件保留，仍可下載
//! - 下載：`Content-Disposition: attachment` 帶原始檔名（`filename*` 為 UTF-8，支援中文）；
//!   附件不存在或已標記刪除 → `404 NOT_FOUND`。未綁定的附件也可以下載（上傳後、存檔前預覽）

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;

use crate::Id;

/// 上傳時放檔案的 multipart 欄位名稱。
pub const UPLOAD_FIELD: &str = "file";

/// 檔名的字元數上限。
pub const MAX_FILE_NAME_CHARS: usize = 255;

/// 未綁定的附件保留的時數，超過後清理。
pub const UNBOUND_RETENTION_HOURS: i64 = 24;

/// 沒有或無法辨識 MIME 時使用的值。
pub const DEFAULT_MIME_TYPE: &str = "application/octet-stream";

/// 上傳成功的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Attachment {
    pub id: Id,
    /// 原始檔名（已去除路徑）。
    pub file_name: String,
    pub mime_type: String,
    /// 位元組數。
    #[specta(type = f64)]
    pub size_bytes: u64,
    /// 檔案內容的 SHA-256（小寫 16 進位）。
    pub sha256: String,
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub created_at: OffsetDateTime,
}

/// 整理上傳的檔名：取最後一個 `/` 或 `\` 之後的部分、控制字元換成空白、去除前後空白，
/// 超過 [`MAX_FILE_NAME_CHARS`] 個字元時保留副檔名截斷。結果為空字串時回傳 `None`。
pub fn sanitize_file_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = cleaned.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if name.chars().count() <= MAX_FILE_NAME_CHARS {
        return Some(name.to_owned());
    }
    // 保留副檔名（最多 20 個字元），其餘截斷
    let ext = name
        .rfind('.')
        .map(|i| &name[i..])
        .filter(|e| e.chars().count() <= 20)
        .unwrap_or("");
    let keep = MAX_FILE_NAME_CHARS - ext.chars().count();
    let stem: String = name.chars().take(keep).collect();
    Some(format!("{}{ext}", stem.trim_end()))
}

/// 整理 MIME：轉小寫，必須是 `type/subtype`（RFC 6838 的字元，不含參數）；不符合時為
/// [`DEFAULT_MIME_TYPE`]。參數（`; charset=…`）去除。
pub fn normalize_mime_type(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return DEFAULT_MIME_TYPE.to_owned();
    };
    let essence = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let valid_part = |s: &str| {
        !s.is_empty()
            && s.len() <= 127
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match essence.split_once('/') {
        Some((t, sub)) if valid_part(t) && valid_part(sub) => essence,
        _ => DEFAULT_MIME_TYPE.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_strips_path_and_control_chars() {
        assert_eq!(
            sanitize_file_name("報價單.pdf").as_deref(),
            Some("報價單.pdf")
        );
        assert_eq!(
            sanitize_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            sanitize_file_name(r"C:\Users\王小明\照片 1.jpg").as_deref(),
            Some("照片 1.jpg")
        );
        assert_eq!(
            sanitize_file_name(" a\r\nb.txt ").as_deref(),
            Some("a  b.txt")
        );
        for bad in ["", "  ", "dir/", "..", ".", "\n"] {
            assert_eq!(sanitize_file_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn long_file_name_keeps_extension() {
        let long = format!("{}.xlsx", "表".repeat(300));
        let name = sanitize_file_name(&long).unwrap();
        assert_eq!(name.chars().count(), MAX_FILE_NAME_CHARS);
        assert!(name.ends_with(".xlsx"));
        // 副檔名太長時直接截斷
        let name = sanitize_file_name(&format!("a.{}", "b".repeat(300))).unwrap();
        assert_eq!(name.chars().count(), MAX_FILE_NAME_CHARS);
    }

    #[test]
    fn mime_type_normalized() {
        assert_eq!(normalize_mime_type(Some("image/PNG")), "image/png");
        assert_eq!(
            normalize_mime_type(Some("text/plain; charset=utf-8")),
            "text/plain"
        );
        assert_eq!(
            normalize_mime_type(Some(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            )),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        for bad in [
            None,
            Some(""),
            Some("text"),
            Some("a/b/c"),
            Some("text/plain\r\nX: y"),
        ] {
            assert_eq!(normalize_mime_type(bad), DEFAULT_MIME_TYPE, "{bad:?}");
        }
    }

    #[test]
    fn json_shape() {
        let a = Attachment {
            id: Id::nil(),
            file_name: "a.txt".into(),
            mime_type: "text/plain".into(),
            size_bytes: 3,
            sha256: "abc".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000000",
                "file_name": "a.txt",
                "mime_type": "text/plain",
                "size_bytes": 3,
                "sha256": "abc",
                "created_at": "1970-01-01T00:00:00Z",
            })
        );
    }
}
