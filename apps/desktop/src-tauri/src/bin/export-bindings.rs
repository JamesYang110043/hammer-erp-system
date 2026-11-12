//! 重新產生 `apps/desktop/src/bindings.ts`（`pnpm gen:bindings`）。

use std::process::ExitCode;

fn main() -> ExitCode {
    match hammer_desktop_lib::export_bindings(hammer_desktop_lib::BINDINGS_PATH) {
        Ok(()) => {
            println!("已產生 {}", hammer_desktop_lib::BINDINGS_PATH);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("產生 TypeScript 綁定失敗：{err}");
            ExitCode::FAILURE
        }
    }
}
