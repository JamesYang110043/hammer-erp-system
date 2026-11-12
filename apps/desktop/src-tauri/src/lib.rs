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

use std::path::Path;

use specta_typescript::Typescript;
use tauri::Manager;
use tauri_specta::{ErrorHandlingMode, collect_commands};

use crate::api_client::{ApiClient, CLIENT_VERSION};
use crate::state::AppState;

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
        ])
        .constant("CLIENT_VERSION", CLIENT_VERSION)
        // command 回傳 `{ status: "ok", data } | { status: "error", error }`，錯誤有型別
        .error_handling(ErrorHandlingMode::Result)
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
            app.manage(AppState::new(ApiClient::new()?, settings_path));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 啟動失敗");
}

#[cfg(test)]
mod tests {
    use super::*;

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
