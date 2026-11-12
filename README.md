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
├── deploy/             # docker-compose（PostgreSQL）
└── docs/
```

模組相依方向（不可反向）：`server → core → db`、`core → formula → shared`，詳見 architecture.md §4。

## 開發環境啟動

### 1. 安裝 Docker

- macOS：安裝 [Docker Desktop for Mac](https://docs.docker.com/desktop/setup/install/mac-install/)
- Windows：安裝 [Docker Desktop for Windows](https://docs.docker.com/desktop/setup/install/windows-install/)（使用 WSL 2 後端）

安裝後開啟 Docker Desktop，確認 `docker compose version` 可以執行。

### 2. 複製 `.env`

`.env` 不納入版本控制，請從範本複製。兩個檔案的帳號密碼要一致。

macOS / Linux：

```bash
cp deploy/.env.example deploy/.env
cp .env.example .env
```

Windows（PowerShell）：

```powershell
Copy-Item deploy\.env.example deploy\.env
Copy-Item .env.example .env
```

### 3. 啟動 PostgreSQL

```bash
cd deploy
docker compose up -d
docker compose ps        # STATUS 顯示 healthy 代表已就緒
```

會建立兩個資料庫：

| 資料庫 | 用途 |
|---|---|
| `hammer_erp` | 開發用 |
| `hammer_erp_test` | 測試用 |

PostgreSQL 只綁定 `127.0.0.1:5432`，其他電腦無法連線。資料存在具名 volume `hammer-erp_pgdata`，`docker compose down` 後再 `up` 資料仍會保留；要清空資料請執行 `docker compose down -v`（**會刪除所有資料**）。

> 初始化腳本（`deploy/initdb/`）只在 volume 為空時執行。修改 `deploy/.env` 的帳號密碼後，需要 `docker compose down -v` 重建才會生效。

### 4. 連線測試

本機有安裝 `psql` 時（密碼見 `deploy/.env`）：

```bash
psql "postgres://hammer:change_me_dev_password@127.0.0.1:5432/hammer_erp" -c "select version();"
```

沒有安裝 `psql` 時，可以使用容器內建的：

```bash
cd deploy
docker compose exec db psql -U hammer -d hammer_erp -c "select version();"
```

### 停止

```bash
cd deploy
docker compose down
```

## 開發

需要安裝 [rustup](https://rustup.rs/)；工具鏈版本由 `rust-toolchain.toml` 指定。

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
