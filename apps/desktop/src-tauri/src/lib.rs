//! Hammer ERP 桌面程式（Tauri 2）的 Rust 端。
//!
//! 資料流：React 介面 ──invoke──▶ Tauri command ──reqwest──▶ API Server
//! （`docs/architecture.md` §3.3）。
//!
//! 相依方向：`desktop(src-tauri) → shared, formula`（§4），不可依賴 server、core、db。

pub mod api_client;
pub mod commands;
pub mod error;
pub mod settings;
pub mod state;
pub mod token_store;

use std::collections::BTreeMap;
use std::path::Path;

use hammer_shared::auth::MIN_PASSWORD_CHARS;
use hammer_shared::schema::MAX_KEY_LEN;
use hammer_shared::{audit, permission};

use specta_typescript::Typescript;
use tauri::Manager;
use tauri_specta::{ErrorHandlingMode, collect_commands};

use crate::api_client::{ApiClient, CLIENT_VERSION};
use crate::state::AppState;
use crate::token_store::KeyringTokenStore;

/// 產生的 TypeScript 綁定檔位置（`apps/desktop/src/bindings.ts`）。
pub const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings.ts");

const BINDINGS_HEADER: &str =
    "// 此檔案由 tauri-specta 自動產生，請勿手動修改。修改 Rust 型別後執行 `pnpm gen:bindings`。";

/// 註冊所有 commands 與要匯出的型別。
pub fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::health_check,
            commands::get_server_url,
            commands::set_server_url,
            commands::setup_status,
            commands::setup_admin,
            commands::login,
            commands::logout,
            commands::current_user,
            commands::change_password,
            commands::list_roles,
            commands::list_users,
            commands::create_user,
            commands::update_user,
            commands::reset_user_password,
            commands::unlock_user,
            commands::delete_user,
            commands::list_audit_logs,
            commands::list_entities,
            commands::get_entity,
            commands::create_entity,
            commands::create_field,
            commands::update_field,
            commands::deactivate_field,
            commands::reorder_fields,
            commands::change_field_type,
            commands::list_records,
            commands::delete_record,
            commands::list_layouts,
            commands::get_default_layout,
            commands::create_layout,
            commands::update_layout,
            commands::delete_layout,
        ])
        .constant("CLIENT_VERSION", CLIENT_VERSION)
        .constant("PERMISSIONS", permission_constants())
        // `usize` 在 TypeScript 可能超出 number 範圍，specta 不允許匯出；數值很小，轉成 u32
        .constant("MIN_PASSWORD_CHARS", MIN_PASSWORD_CHARS as u32)
        // 操作紀錄的動作代碼（`hammer_shared::audit::ALL`），前端篩選選單使用
        .constant("AUDIT_ACTIONS", audit::ALL)
        // 資料類別、欄位代碼的長度上限（`hammer_shared::schema::MAX_KEY_LEN`），前端即時檢查使用
        .constant("MAX_KEY_LEN", MAX_KEY_LEN as u32)
        // 帳號管理
        .typ::<hammer_shared::user::Role>()
        .typ::<hammer_shared::user::User>()
        .typ::<hammer_shared::user::CreateUserRequest>()
        .typ::<hammer_shared::user::UpdateUserRequest>()
        .typ::<hammer_shared::user::ResetPasswordRequest>()
        .typ::<hammer_shared::user::DeleteUserQuery>()
        // 分頁（列表 API 共用）
        .typ::<hammer_shared::pagination::Page<hammer_shared::audit::AuditLogEntry>>()
        // 操作紀錄
        .typ::<hammer_shared::audit::AuditLogEntry>()
        .typ::<hammer_shared::audit::AuditLogQuery>()
        // 資料類別與欄位定義
        .typ::<hammer_shared::schema::DataType>()
        .typ::<hammer_shared::schema::EntityKind>()
        .typ::<hammer_shared::schema::EntityStorage>()
        .typ::<hammer_shared::schema::FieldStorage>()
        .typ::<hammer_shared::schema::Validation>()
        .typ::<hammer_shared::schema::SelectOption>()
        .typ::<hammer_shared::schema::EntityDefinition>()
        .typ::<hammer_shared::schema::FieldDefinition>()
        .typ::<hammer_shared::schema::CreateEntityRequest>()
        .typ::<hammer_shared::schema::UpdateEntityRequest>()
        .typ::<hammer_shared::schema::CreateFieldRequest>()
        .typ::<hammer_shared::schema::UpdateFieldRequest>()
        .typ::<hammer_shared::schema::DeactivateFieldRequest>()
        .typ::<hammer_shared::schema::ChangeFieldTypeRequest>()
        .typ::<hammer_shared::schema::EntityDetail>()
        .typ::<hammer_shared::schema::ReorderFieldsRequest>()
        .typ::<hammer_shared::schema::FieldOrderItem>()
        // 自訂資料類別的資料
        .typ::<hammer_shared::record::Record>()
        .typ::<hammer_shared::record::FilterOp>()
        .typ::<hammer_shared::record::RecordFilter>()
        .typ::<hammer_shared::record::RecordSort>()
        .typ::<hammer_shared::record::RecordListQuery>()
        .typ::<hammer_shared::pagination::Page<hammer_shared::record::Record>>()
        // 列表版面
        .typ::<hammer_shared::layout::LayoutColumn>()
        .typ::<hammer_shared::layout::LayoutConfig>()
        .typ::<hammer_shared::layout::ViewLayout>()
        .typ::<hammer_shared::layout::CreateViewLayoutRequest>()
        .typ::<hammer_shared::layout::UpdateViewLayoutRequest>()
        .typ::<hammer_shared::layout::DeleteViewLayoutQuery>()
        .typ::<hammer_shared::layout::LayoutSource>()
        .typ::<hammer_shared::layout::DefaultViewLayout>()
        // command 回傳 `{ status: "ok", data } | { status: "error", error }`，錯誤有型別
        .error_handling(ErrorHandlingMode::Result)
}

/// 權限代碼（`hammer_shared::permission`），前端以 `PERMISSIONS.USER_MANAGE` 等使用。
fn permission_constants() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("RECORD_READ", permission::RECORD_READ),
        ("RECORD_WRITE", permission::RECORD_WRITE),
        ("RECORD_POST", permission::RECORD_POST),
        ("DATA_EXPORT", permission::DATA_EXPORT),
        ("SCHEMA_MANAGE", permission::SCHEMA_MANAGE),
        ("USER_MANAGE", permission::USER_MANAGE),
        ("SETTINGS_MANAGE", permission::SETTINGS_MANAGE),
        ("AUDIT_READ", permission::AUDIT_READ),
    ])
}

/// 產生 TypeScript 綁定檔。
pub fn export_bindings(path: impl AsRef<Path>) -> Result<(), specta_typescript::Error> {
    specta_builder().export(Typescript::default().header(BINDINGS_HEADER), path)
}

pub fn run() {
    let builder = specta_builder();

    // 開發時每次啟動都重新產生綁定檔
    #[cfg(debug_assertions)]
    if let Err(err) = export_bindings(BINDINGS_PATH) {
        eprintln!("產生 TypeScript 綁定失敗：{err}");
    }

    tauri::Builder::default()
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let settings_path = app.path().app_config_dir()?.join(settings::SETTINGS_FILE);
            app.manage(AppState::new(
                ApiClient::new()?,
                settings_path,
                Box::new(KeyringTokenStore),
            ));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 啟動失敗");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_constants_cover_all_codes() {
        let mut exported: Vec<_> = permission_constants().into_values().collect();
        let mut all = permission::ALL.to_vec();
        exported.sort_unstable();
        all.sort_unstable();
        assert_eq!(exported, all);
    }

    /// `src/bindings.ts` 必須與目前的 Rust 型別一致；不一致時執行 `pnpm gen:bindings`。
    #[test]
    fn bindings_are_up_to_date() {
        let tmp = std::env::temp_dir().join(format!("hammer-bindings-{}.ts", std::process::id()));
        export_bindings(&tmp).unwrap();
        let generated = std::fs::read_to_string(&tmp).unwrap();
        let _ = std::fs::remove_file(&tmp);
        let committed = std::fs::read_to_string(BINDINGS_PATH)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            generated == committed,
            "src/bindings.ts 不是最新的，請在 apps/desktop 執行 `pnpm gen:bindings`"
        );
    }
}
