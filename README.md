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
├── apps/desktop/       # Tauri 桌面程式（src-tauri：Rust 端；src：React 前端）
├── deploy/             # docker-compose（PostgreSQL、API Server）、Dockerfile.server
└── docs/
```

模組相依方向（不可反向）：`server → core → db`、`core → formula → shared`、`desktop(src-tauri) → shared, formula`，詳見 architecture.md §4。

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

## 單機部署

第一階段把 PostgreSQL 和 API Server 都放在同一台電腦的 Docker 中執行，桌面程式連到 `http://localhost:8080`（architecture.md §3.1）。

`deploy/docker-compose.yml` 用 profile 區分兩種用法：

| 用途 | 指令（在 `deploy/` 目錄） | 啟動的服務 |
|---|---|---|
| 開發 | `docker compose up -d` | 只有 `db`；API Server 用 `cargo run` 啟動 |
| 單機部署 | `docker compose --profile full up -d --build` | `db` 與 `server` |

兩種用法都會占用 `8080` 埠，同時執行 `cargo run -p hammer-server` 與 `server` 容器時，後啟動的那一個會無法監聽。

### 1. 準備

1. 安裝 Docker（見上方「開發環境啟動」第 1 步）。
2. 複製並修改 `deploy/.env`：

   ```bash
   cp deploy/.env.example deploy/.env
   ```

   正式使用前請改掉 `POSTGRES_PASSWORD`。帳號或密碼含 URL 特殊字元（`@`、`:`、`/` 等）時，要另外處理 URL 編碼；建議只使用英數字。單機部署不需要根目錄的 `.env`。

### 2. 建置並啟動

```bash
cd deploy
docker compose --profile full up -d --build
docker compose --profile full ps      # db 與 server 的 STATUS 都顯示 healthy 代表已就緒
curl localhost:8080/api/v1/health
# {"status":"ok","version":"0.1.0","database":"ok"}
```

- 第一次建置需要下載並編譯所有 Rust 相依套件，要花數分鐘。之後只改程式碼時，相依套件會沿用 Docker 快取（cargo-chef）。
- `server` 等到 `db` 的 healthcheck 通過才會啟動，啟動時會自動套用 migrations。
- 建置使用 `SQLX_OFFLINE=true`，讀取 `.sqlx/` 中的查詢型別資訊，不需要連線資料庫。

### 3. 連線

桌面程式的伺服器網址預設就是 `http://localhost:8080`，不需要修改。可以在「連線設定」按「測試連線」確認。

### 網路與資料

| 項目 | 設定 |
|---|---|
| API Server | 預設只綁定 `127.0.0.1:8080`。之後改用 Tailscale 時，在 `deploy/.env` 設定 `HAMMER_PUBLISH_ADDR`（architecture.md §3.2） |
| PostgreSQL | 只綁定 `127.0.0.1:5432`；`server` 容器透過 Docker 內部網路以 `db:5432` 連線。其他電腦無法連到資料庫 |
| 資料庫資料 | 具名 volume `hammer-erp_pgdata` |
| 附件 | 具名 volume `hammer-erp_attachments`（容器內 `/var/lib/hammer/attachments`） |
| log | JSON 格式，用 `docker compose logs -f server` 查看；等級以 `deploy/.env` 的 `HAMMER_LOG_LEVEL` 設定 |

### 開機自動啟動

兩個服務都設定 `restart: unless-stopped`：電腦重新開機、Docker 啟動後會自動恢復，除非之前用 `docker compose stop` 或 `down` 手動停止。需要 Docker 本身在開機時啟動：

- Docker Desktop（macOS / Windows）：Settings → General → 勾選「Start Docker Desktop when you sign in to your computer」。電腦也要設定為自動登入，否則要登入後才會啟動。
- Linux：`sudo systemctl enable docker`

### 更新與停止

```bash
cd deploy
git pull
docker compose --profile full up -d --build   # 重新建置並替換 server 容器，資料保留
docker compose --profile full down            # 停止並移除容器，資料保留
```

`docker compose --profile full down -v` 會連同資料庫與附件的 volume 一起刪除（**會刪除所有資料**）。

## 開發

需要安裝 [rustup](https://rustup.rs/)；工具鏈版本由 `rust-toolchain.toml` 指定。

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### 啟動 API Server

先啟動 PostgreSQL 並複製 `.env`（見上方「開發環境啟動」），然後：

```bash
cargo run -p hammer-server
curl localhost:8080/api/v1/health
# {"status":"ok","version":"0.1.0","database":"ok"}
```

啟動時會自動套用 migrations。按 Ctrl+C（或送 SIGTERM）時，會等進行中的請求完成再關閉。

設定的優先順序是：環境變數 > 設定檔（以 `HAMMER_CONFIG` 指定路徑，範例見 `crates/server/config.example.toml`）> 預設值。

| 環境變數 | 預設值 | 說明 |
|---|---|---|
| `DATABASE_URL` | （必填） | PostgreSQL 連線字串 |
| `HAMMER_LISTEN_ADDR` | `127.0.0.1:8080` | 監聽位址 |
| `HAMMER_LOG_LEVEL` | `info` | log 等級（tracing `EnvFilter` 語法） |
| `HAMMER_LOG_FORMAT` | 開發版 `pretty`、release 版 `json` | log 格式 |
| `HAMMER_ATTACHMENTS_DIR` | `data/attachments` | 附件存放目錄，啟動時自動建立 |
| `HAMMER_BODY_LIMIT_BYTES` | `10485760` | Request body 大小上限 |

`GET /api/v1/health` 不需要登入：資料庫正常時回傳 `200`，資料庫異常時回傳 `503`（`"database":"error"`），伺服器不會因此停止。
所有錯誤都以統一格式回傳（`{"code":"NOT_FOUND","message":"…"}`），每個回應都有 `x-request-id` 標頭，與 log 中的 `request_id` 對應。
帶 `X-Client-Version` 標頭且版本低於 `MIN_CLIENT_VERSION`（`crates/shared/src/constants.rs`）時回傳 `426`；沒帶標頭時放行。

### 資料庫 migration

Migration 放在 `crates/db/migrations/`，檔名格式為 `<4 位數版本>_<英文描述>.sql`（例如 `0002_users.sql`）。檔案在編譯時嵌入，以 `hammer_db::run_migrations` 套用（依 architecture.md §11，API Server 啟動時自動執行）；已套用過的會略過。

共通欄位（`created_at`、`updated_at`、`version` 等）與 `custom_data` 的慣例，寫在 `0001_foundation.sql` 的註解。有 `updated_at` 的資料表要掛上 `set_updated_at()` trigger。

### 整合測試

`crates/db/tests/` 與 `crates/server/tests/` 中需要資料庫的測試使用 `#[sqlx::test]`，每個測試會建立獨立的暫時資料庫、套用 migration，結束後刪除。需要先啟動 PostgreSQL，並以 `DATABASE_URL` 指定連線（帳號需有 CREATEDB 權限；docker compose 建立的帳號已具備）。建議指向測試用資料庫，避免在開發資料庫留下測試用的 schema：

```bash
DATABASE_URL=postgres://hammer:change_me_dev_password@127.0.0.1:5432/hammer_erp_test cargo test --workspace
```

沒有設定環境變數時，會讀取根目錄 `.env` 的 `DATABASE_URL`；兩者都沒有時，資料庫測試會失敗。

### sqlx offline mode

使用 `sqlx::query!` 等巨集時，編譯期需要查詢的型別資訊。這些資訊存在根目錄 `.sqlx/`，納入版本控制，因此**不需要連線資料庫也能建置**：

```bash
SQLX_OFFLINE=true cargo build --workspace
```

新增或修改查詢後，要在資料庫已套用最新 migration 的狀態下重新產生 `.sqlx/` 並一起 commit：

```bash
cargo install sqlx-cli --version 0.9.0 --no-default-features --features postgres,rustls --locked   # 只需安裝一次
cargo sqlx migrate run --source crates/db/migrations    # 讓 DATABASE_URL 的資料庫套用最新 migration
cargo sqlx prepare --workspace                          # 產生 .sqlx/
cargo sqlx prepare --workspace --check                  # 檢查 .sqlx/ 是否為最新（PR 前執行）
```

## 桌面程式（`apps/desktop`）

Tauri 2 + React + TypeScript + Ant Design 5。前端不直接呼叫 HTTP API，一律透過 Tauri command（Rust 端用 reqwest 呼叫 API Server，architecture.md §3.3）。

需要安裝：

- [rustup](https://rustup.rs/)
- Node.js 22.22 以上與 pnpm（`corepack enable` 後會依 `package.json` 的 `packageManager` 使用指定版本）
- Tauri 的系統相依套件：macOS 需要 Xcode Command Line Tools；Windows 需要 Microsoft C++ Build Tools 與 WebView2，詳見 [Tauri 官方說明](https://v2.tauri.app/start/prerequisites/)

```bash
cd apps/desktop
pnpm install
pnpm tauri dev        # 開發模式（會自動啟動 Vite）
pnpm lint             # ESLint + Prettier 檢查
pnpm typecheck        # TypeScript 型別檢查
pnpm format           # 以 Prettier 格式化
```

- 伺服器網址預設 `http://localhost:8080`，可在「連線設定」修改並「測試連線」；設定存在作業系統的 app 設定目錄（`settings.json`）。
- 底部狀態列每 10 秒檢查一次 `/api/v1/health`，顯示連線狀態與伺服器／用戶端版本。
- 每個請求都會帶 `X-Client-Version`（`apps/desktop/src-tauri/Cargo.toml` 的版本）。

### TypeScript 型別（tauri-specta）

`src/bindings.ts` 由 Rust 型別與 commands 自動產生，**納入版本控制，不要手動修改**。修改 Rust 型別或新增 command 後重新產生：

```bash
cd apps/desktop
pnpm gen:bindings     # pnpm tauri dev 啟動時也會自動重新產生
pnpm typecheck        # 型別不符時會在這裡報錯
```

`cargo test --workspace` 中的 `bindings_are_up_to_date` 會檢查 `bindings.ts` 是否為最新。
