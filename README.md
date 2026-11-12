# Hammer ERP

給小型團隊（5 人以內）使用的桌面 ERP 系統，涵蓋進出貨、客戶、廠商、發票、財務與系統管理。

- 桌面程式：Tauri 2 + React + TypeScript + Ant Design
- API Server：Rust（axum）
- 資料庫：PostgreSQL

架構與設計決策請見 [docs/architecture.md](docs/architecture.md)，開發流程請見 [docs/issue-workflow.md](docs/issue-workflow.md)。

## 目錄結構

```
hammer-erp-system/
├── Cargo.toml          # Rust workspace
├── crates/
│   ├── shared/         # 前後端共用：資料型別、錯誤碼、權限常數
│   ├── formula/        # 公式引擎
│   ├── core/           # 商業邏輯
│   ├── db/             # sqlx 資料存取、migrations
│   └── server/         # axum API Server
├── apps/desktop/       # Tauri 桌面程式（尚未建立）
├── deploy/             # docker-compose（尚未建立）
└── docs/
```

模組相依方向（不可反向）：`server → core → db`、`core → formula → shared`，詳見 architecture.md §4。

## 開發

需要安裝 [rustup](https://rustup.rs/)；工具鏈版本由 `rust-toolchain.toml` 指定。

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
