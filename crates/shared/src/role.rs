//! 內建角色代碼（`docs/architecture.md` D6、§6.1）。
//!
//! 資料庫 `roles.code` 存的就是這些字串。角色擁有哪些權限由 `role_permissions` 決定。

/// 管理者。
pub const ADMIN: &str = "admin";
/// 可編輯者。
pub const EDITOR: &str = "editor";
/// 唯讀者。
pub const VIEWER: &str = "viewer";

/// 所有內建角色代碼。
pub const ALL: &[&str] = &[ADMIN, EDITOR, VIEWER];
