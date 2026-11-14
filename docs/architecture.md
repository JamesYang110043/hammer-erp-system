# Hammer ERP 系統架構文件

> 狀態：草案 v0.1（2026-11-12）
> 本文件記錄已確定的架構決策與資料庫設計，作為後續所有開發 issue 的共同依據。
> 標示「⏳ 待討論」的部分尚未定案，實作到該模組前需再確認。

---

## 1. 系統概述

Hammer ERP 是給小型團隊（5 人以內）使用的桌面 ERP 系統，涵蓋以下模組：

| 模組 | 範圍 |
|---|---|
| 進出貨管理 | 採購、進貨、訂單、出貨、退貨、批號庫存（先進先出） |
| 客戶管理 | 客戶主檔、聯絡人 |
| 廠商管理 | 廠商主檔、聯絡人 |
| 發票管理 | 內部紀錄（⏳ 細節待討論） |
| 財務管理 | ⏳ 細節待討論 |
| 系統管理 | 帳號、權限、欄位與資料類別客製化、操作紀錄 |

---

## 2. 決策總表

| # | 項目 | 決定 | 理由 |
|---|---|---|---|
| D1 | 架構 | 客戶端與伺服器分離：Tauri 桌面程式 → API Server（axum）→ PostgreSQL | 權限必須在伺服器端強制執行；客戶端不得持有資料庫帳密 |
| D2 | 部署 | 第一階段單機（伺服器與客戶端在同一台電腦），之後透過 Tailscale 開放多人連線 | 單機轉多人只需修改伺服器網址，程式碼不需改動 |
| D3 | 資料庫 | 一開始就用 PostgreSQL（不用 SQLite） | 需要 JSONB、運算式索引；避免中途更換資料庫導致重寫資料層 |
| D4 | 桌面框架 | Tauri 2，可在 macOS 和 Windows 執行 | 網頁生態的表格與表單元件成熟，適合 ERP |
| D5 | 前端 | React + TypeScript + Ant Design | Ant Design 的表格、表單、中文語系完整 |
| D6 | 權限 | 三種角色：管理者、可編輯者、唯讀者；資料表採「角色對應權限」設計 | 保留未來擴充彈性 |
| D7 | 客製化 | 欄位定義引擎 + 自訂資料類別 + 公式欄位，只有管理者可修改 | 需求：每個欄位都能高度客製化 |
| D8 | 自訂資料類別範圍 | 第一版只記錄資料，不影響庫存與財務 | 控制複雜度 |
| D9 | 金額計算 | 全程使用 `rust_decimal`，禁止使用浮點數 | 避免金額誤差 |
| D10 | 庫存 | 單一倉庫；所有實體商品都有批號；出貨時預設先進先出，可手動指定批號；不記有效期限 | 需求 |
| D11 | 幣別 | 本位幣是台幣；進貨支援外幣，依進貨當天匯率把成本換算成台幣 | 需求 |
| D12 | 同時編輯 | 樂觀鎖定（每筆資料有 `version` 欄位） | 成本低，可避免互相覆蓋 |
| D13 | 語系與時區 | 只做繁體中文介面，時區 Asia/Taipei | 需求範圍 |

---

## 3. 系統架構

### 3.1 部署架構

```
【第一階段：單機】
同一台電腦
┌────────────────────────────────┐
│ Tauri 桌面程式                  │
│   │ http://localhost:8080       │
│   ▼                             │
│ API Server (axum)  ── Docker    │
│   │                             │
│   ▼                             │
│ PostgreSQL         ── Docker    │
└────────────────────────────────┘

【第二階段：多人（Tailscale）】
伺服器主機                                      其他電腦（最多約 4 台）
┌──────────────────────────┐                 ┌──────────────┐
│ API Server (axum)        │◀── HTTPS ───────│ Tauri 桌面程式│
│   │                      │   (Tailscale)   └──────────────┘
│   ▼                      │
│ PostgreSQL（只開放本機） │
│ 每日 pg_dump 備份        │
└──────────────────────────┘
```

### 3.2 Tailscale 階段的規則

- 對外只開放 API Server 的埠號。PostgreSQL（5432）只綁定本機或 Docker 內部網路，**不可**讓 Tailscale 網路連到。
- 用 `tailscale cert` 取得 MagicDNS 網域（`*.ts.net`）的正式 HTTPS 憑證。
- 用 Tailscale ACL 限定只有公司的裝置能連線。
- 伺服器主機要設定不休眠、開機自動啟動 Docker。
- ⏳ Tailscale 方案的人數上限、備份要存到哪裡，到時再確認。

### 3.3 桌面程式內部的資料流

```
React 介面 ──invoke──▶ Tauri command (Rust) ──reqwest──▶ API Server
                          │
                          ├─ 登入 token 存在作業系統鑰匙圈（keyring crate）
                          ├─ 直接使用 crates/shared 的資料型別
                          └─ 直接使用 crates/formula 做公式即時預覽
```

- 前端不直接呼叫 HTTP API，一律透過 Tauri command。
- 登入 token 只留在 Rust 端，不傳給前端；鑰匙圈以伺服器網址區分，切換伺服器時各自保留登入狀態。需要登入的請求收到 `401` 時，Rust 端清除 token 並回傳 `sessionExpired` 錯誤，前端回到登入頁。
- Rust 型別用 `tauri-specta` 自動產生 TypeScript 型別，確保前後端欄位一致。

---

## 4. 專案結構

```
hammer-erp-system/
├── Cargo.toml                 # workspace
├── crates/
│   ├── shared/                # 前後端共用：資料型別、錯誤碼、權限常數
│   ├── formula/               # 公式引擎：解析器、Decimal 計算、相依性分析
│   ├── core/                  # 商業邏輯：單據流程、庫存 FIFO、匯率換算、權限判斷
│   ├── db/                    # sqlx 資料存取、migrations
│   └── server/                # axum API Server
├── apps/
│   └── desktop/
│       ├── src-tauri/         # Tauri Rust 端（API client、commands）
│       └── src/               # React + Ant Design 前端
├── deploy/
│   └── docker-compose.yml     # PostgreSQL + API Server
└── docs/
    └── architecture.md
```

模組之間的相依方向（不可反向）：

```
server ──▶ core ──▶ db
   │         │
   │         ├──▶ formula ──▶ shared
   │         └──▶ shared
   └──▶ shared

desktop(src-tauri) ──▶ shared, formula
```

---

## 5. 技術選型

| 層 | 套件 |
|---|---|
| 非同步執行環境 | tokio |
| HTTP Server | axum、tower-http |
| 資料庫存取 | sqlx（PostgreSQL；啟用 offline mode，`.sqlx/` 目錄納入版本控制） |
| 金額 | rust_decimal |
| 密碼雜湊 | argon2（Argon2id） |
| 主鍵 | uuid（v7） |
| 時間 | time（不使用 chrono，全專案統一）。Asia/Taipei 固定 UTC+8、無日光節約，用固定位移即可，不需時區資料庫；工具函式在 `crates/shared` 的 `time` 模組 |
| 錯誤處理 | thiserror（函式庫）、anyhow（執行檔） |
| 日誌 | tracing |
| HTTP Client | reqwest（rustls） |
| 型別產生 | tauri-specta |
| 匯出 Excel | rust_xlsxwriter |
| 前端 | React、TypeScript、Vite、Ant Design 5、TanStack Query、React Router、dayjs、dnd-kit（拖曳排序） |
| Tauri 外掛 | updater（自動更新） |

---

## 6. 認證與權限

### 6.1 角色與權限

| 權限代碼 | 說明 | 管理者 | 可編輯者 | 唯讀者 |
|---|---|:-:|:-:|:-:|
| `record.read` | 查看業務資料 | ✅ | ✅ | ✅ |
| `record.write` | 新增、修改業務資料 | ✅ | ✅ | ❌ |
| `record.post` | 過帳、作廢單據 | ✅ | ✅ | ❌ |
| `data.export` | 匯出 Excel、列印 | ✅ | ✅ | ✅ |
| `schema.manage` | 欄位與資料類別客製化 | ✅ | ❌ | ❌ |
| `user.manage` | 帳號管理 | ✅ | ❌ | ❌ |
| `settings.manage` | 系統設定（單號規則、匯率等） | ✅ | ❌ | ❌ |
| `audit.read` | 查看操作紀錄 | ✅ | ❌ | ❌ |

> 匯率維護：目前歸在 `settings.manage`。如果可編輯者也需要輸入匯率，再調整。

### 6.2 安全規則

- 密碼用 Argon2id 雜湊後儲存，不保存明文。
- 密碼至少 10 個字元（以字元計，不是 bytes），不另外要求大小寫、數字或符號。
- 連續登入失敗 5 次，鎖定帳號 15 分鐘（數值可在系統設定調整）。
- 新帳號或管理者重設密碼後，第一次登入強制修改密碼。
- Session 用隨機 token，資料庫只存 token 的 SHA-256 雜湊；閒置 480 分鐘（8 小時）後自動失效（數值可在系統設定調整）。
- Session 沒有絕對有效期限：`expires_at` = `last_seen_at` + 閒置時間，每次使用時一起延長；持續使用就不會被登出。
- 以上數值存在 `settings`，預設值由 migration 寫入：`security.max_failed_logins` = 5、`security.lockout_minutes` = 15、`security.session_idle_minutes` = 480。
- 每個 API 都在伺服器端（`core`）檢查權限。前端隱藏按鈕只是輔助。
- 系統至少要保留一個有效的管理者帳號，不能把最後一個管理者刪除或降級。

---

## 7. 客製化引擎

### 7.1 兩種資料類別

| | 系統資料類別 | 自訂資料類別 |
|---|---|---|
| 例子 | 客戶、商品、訂單、進貨單 | 管理者自建的「維修單」 |
| 儲存 | 專屬資料表 + `custom_data` JSONB | 通用資料表 `custom_records.data` JSONB |
| 可否刪除 | 不可 | 可（軟刪除） |
| 影響庫存與財務 | 依系統邏輯 | 第一版不影響（D8） |

### 7.2 欄位的兩個層級

| | 系統欄位（`is_system = true`） | 自訂欄位 |
|---|---|---|
| 儲存位置 | 資料表的實體欄位 | `custom_data` / `data` JSONB |
| 可修改 | 顯示名稱、顯示或隱藏、排序、欄寬、說明文字；必填與預設值在不違反系統規則下可改 | 全部屬性 |
| 不可修改 | 資料型態、欄位代碼，也不可刪除 | 欄位代碼（建立後就固定） |
| 刪除 | 不可 | 只能停用（`is_active = false`），資料保留 |

### 7.3 欄位資料型態

| data_type | 說明 | JSON 儲存格式 |
|---|---|---|
| `text` | 單行文字 | string |
| `long_text` | 多行文字 | string |
| `integer` | 整數 | number |
| `decimal` | 小數（指定小數位數） | **string**（例如 `"12.3400"`），避免精度損失 |
| `money` | 金額（指定幣別或跟隨單據幣別） | **string** |
| `date` | 日期 | string `YYYY-MM-DD` |
| `datetime` | 日期時間 | string RFC 3339 |
| `boolean` | 勾選 | boolean |
| `select` | 下拉單選 | string（選項值） |
| `multi_select` | 下拉多選 | string[] |
| `relation` | 關聯到其他資料類別 | string（UUID） |
| `file` | 附件 | string（attachments.id） |
| `formula` | 公式（唯讀，由系統計算） | 依 `result_type` 決定 |

### 7.4 欄位型態變更規則

| 從 → 到 | 規則 |
|---|---|
| 任意型態 → `text` / `long_text` | 允許 |
| `text` ↔ `long_text` | 允許 |
| `integer` → `decimal` | 允許 |
| `select` → `multi_select` | 允許 |
| 其他轉換 | 先驗證全部既有資料。只要有一筆轉換失敗就拒絕，並列出失敗的資料 |

- 轉換範圍包含已軟刪除的資料，在同一個交易內完成資料轉換與欄位定義更新。
- 「允許」指型態本身一定能轉換；轉換後的值仍須符合新的設定（選項、小數位數、驗證規則），「不可重複」的欄位轉換後也不可重複。
- 寫入資料時以 `FOR SHARE` 鎖定資料類別，變更型態時以 `FOR UPDATE` 鎖定，避免轉換期間依舊的欄位定義寫入資料。
- 各型態的轉換方式見 `crates/core/src/schema/type_change.rs`。

### 7.5 索引策略

- 每張系統資料表的 `custom_data`，以及 `custom_records.data`，都建 GIN 索引。
- 欄位設為「可搜尋」時，自動建立運算式索引，例如 `((data->>'field_key'))`。
- 欄位設為「不可重複」時，自動建立部分唯一索引：
  `UNIQUE (entity_id, (data->>'field_key')) WHERE deleted_at IS NULL`。
- 建立索引前會先檢查既有資料，有重複就拒絕並列出衝突的資料。
- 自訂資料類別（`custom_records`）的動態索引都加上 `entity_id = '<資料類別 id>'` 條件，限定在單一資料類別：不同資料類別即使有相同的 `field_key`，索引與唯一性也互不影響。
- 索引只存在於**啟用中**的欄位：停用欄位時移除，重新啟用時再建立（不可重複的欄位重新啟用前一樣先檢查重複）。修改欄位定義與建立、移除索引在同一個交易內。
- 索引名稱：`{前綴}_{entity_key}_{field_key}_{雜湊}`，前綴為 `cfs`（可搜尋）或 `cfu`（不可重複）；前綴與代碼的部分超過 54 個字元時截斷，雜湊為兩個代碼的 SHA-256 前 8 碼，名稱最長 63 個字元（PostgreSQL 識別字上限）。規則在 `crates/core/src/schema/index.rs`。
- DDL 無法使用 bind 參數：`field_key` 與索引名稱拼接前一律檢查只含 `a-z`、`0-9`、`_` 並加上引號（`crates/db/src/dynamic_index.rs`）。
- 有重複而無法開啟「不可重複」時回傳 `422 VALIDATION`，`details` 為 `{ "field": "is_unique", "total", "values": [{ "value", "count", "record_ids" }] }`。
- 系統資料表 `custom_data` 的動態索引在 M3 建立系統資料表時處理。

### 7.6 公式引擎（crates/formula）

**語法範例：**

```
{qty} * {unit_price}
ROUND({subtotal} * 0.05, 0)
SUM(lines.amount) + {shipping_fee}
{total} * (1 - {customer.discount_rate})
IF({status} = "open", DAYS(TODAY(), {due_date}), 0)
```

**規格：**

| 項目 | 規格 |
|---|---|
| 欄位引用 | `{field_key}`。公式存的是欄位代碼，所以修改顯示名稱不會讓公式失效；介面上顯示為欄位名稱 |
| 關聯引用 | `{relation_field.field_key}`，最多一層 |
| 子表彙總 | `SUM(child.field_key)`、`AVG`、`COUNT`、`MIN`、`MAX` |
| 運算子 | `+ - * /`、`= <> < <= > >=`、`AND OR NOT`、`&`（字串連接） |
| 函式 | `IF`、`ROUND`、`ABS`、`SUM`、`AVG`、`COUNT`、`MIN`、`MAX`、`TODAY`、`DAYS`、`CONCAT`、`COALESCE` |
| 數值計算 | 全程用 `rust_decimal`（自己寫解析器，不使用以浮點數計算的現成套件） |
| 錯誤 | 除以零、型態不符時，結果為空值，並在欄位顯示錯誤提示，不中斷存檔 |
| 計算時機 | 存檔時由伺服器計算並寫入資料庫（可排序、篩選、用於報表）；桌面端用同一個 crate 即時預覽 |
| 相依性 | 儲存公式時建立相依圖並檢查循環引用，有循環就拒絕儲存；計算順序依拓撲排序 |
| 重新計算 | 修改公式後，管理者可對該資料類別的全部資料執行「重新計算」 |
| 限制 | 公式只能讀資料，不能寫入其他欄位；系統欄位（例如單據金額、稅額）不能用公式覆蓋 |

---

## 8. 資料庫設計

### 8.1 通用慣例

| 項目 | 規則 |
|---|---|
| 主鍵 | `id UUID`（v7，依時間排序） |
| 命名 | 資料表用複數 snake_case；外鍵用 `{單數}_id` |
| 金額 | `NUMERIC(18,2)`（台幣金額顯示時取整數） |
| 外幣金額 | `NUMERIC(18,4)` |
| 單價、單位成本 | `NUMERIC(18,4)` |
| 數量 | `NUMERIC(18,4)` |
| 匯率 | `NUMERIC(18,6)`，代表 1 單位外幣可換多少台幣 |
| 時間 | `TIMESTAMPTZ`；日期用 `DATE` |
| 列舉值 | 用 `TEXT` 加 `CHECK` 限制，比 PostgreSQL ENUM 容易擴充 |

**共通欄位**（以下資料表定義中寫「＋共通欄位」的都包含這些欄位）：

```sql
created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
created_by  UUID REFERENCES users(id),
updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
updated_by  UUID REFERENCES users(id),
deleted_at  TIMESTAMPTZ,              -- 軟刪除
version     INTEGER NOT NULL DEFAULT 1 -- 樂觀鎖定
```

**可客製化的資料表**另外還有：

```sql
custom_data JSONB NOT NULL DEFAULT '{}'
```

### 8.2 單據狀態

```
draft（草稿）──確認──▶ posted（已過帳）──作廢──▶ void（已作廢）
   │                        │
   └── 可自由修改或刪除      └── 不可修改；作廢時自動產生反向庫存異動
```

採購單、訂單這類不影響庫存的單據，使用另一套狀態：
`draft → confirmed → partially_done → closed / void`。

### 8.3 ER 概觀

```
suppliers ─┬─< purchase_orders ─< purchase_order_lines
           └─< goods_receipts ──< goods_receipt_lines ──1:1── inventory_lots
                                                                  │
customers ─┬─< sales_orders ────< sales_order_lines               │
           └─< shipments ───────< shipment_lines ─< lot_allocations >─┘
                                                                  │
products ─────────────────────────────────────< inventory_transactions
```

---

### 8.4 系統模組

#### `roles`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| code | TEXT UNIQUE | `admin` / `editor` / `viewer` |
| name | TEXT | 管理者 / 可編輯者 / 唯讀者 |
| is_system | BOOLEAN | 內建角色不可刪除 |

#### `role_permissions`
| 欄位 | 型態 | 說明 |
|---|---|---|
| role_id | UUID FK → roles | |
| permission | TEXT | 權限代碼，見 6.1 |
| | | PK (role_id, permission) |

#### `users`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| username | TEXT UNIQUE | 登入帳號 |
| display_name | TEXT | 顯示名稱 |
| password_hash | TEXT | Argon2id |
| role_id | UUID FK → roles | |
| is_active | BOOLEAN | 停用後無法登入 |
| must_change_password | BOOLEAN | |
| failed_login_count | INTEGER | |
| locked_until | TIMESTAMPTZ | |
| last_login_at | TIMESTAMPTZ | |
| ＋共通欄位 | | |

#### `sessions`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| user_id | UUID FK → users | |
| token_hash | TEXT UNIQUE | token 的 SHA-256 |
| client_info | TEXT | 裝置名稱與客戶端版本 |
| created_at / last_seen_at / expires_at | TIMESTAMPTZ | `expires_at` = `last_seen_at` + 閒置時間，隨使用延長（§6.2） |
| revoked_at | TIMESTAMPTZ | 登出或被強制登出 |

#### `audit_logs`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| occurred_at | TIMESTAMPTZ | |
| user_id | UUID FK → users | |
| action | TEXT | `create` / `update` / `delete` / `post` / `void` / `login` / `login_failed` / `logout` / `password_change` / `export`（常數在 `hammer_shared::audit`） |
| entity_key | TEXT | 資料類別代碼 |
| record_id | UUID | |
| before_data / after_data | JSONB | 變更前與變更後（只記錄有變動的欄位） |
| client_info | TEXT | |

#### `settings`
| 欄位 | 型態 | 說明 |
|---|---|---|
| key | TEXT PK | 例如 `security.max_failed_logins` |
| value | JSONB | |
| updated_at / updated_by | | |

#### `number_sequences`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_id | UUID FK → entity_definitions | |
| period_key | TEXT | 依單號規則產生，例如 `202611` |
| current_value | INTEGER | 該期間最後取出的流水號 |
| | | UNIQUE (entity_id, period_key) |

> 單號規則存在 `entity_definitions.numbering_pattern`，例如 `SO{yyyyMM}-{0000}` 會產生 `SO202611-0001`。

**單號規則**（解析在 `hammer_core::numbering`）：

| 代碼 | 說明 |
|---|---|
| `{yyyy}`、`{yy}` | 西元年 4 位、末 2 位 |
| `{MM}`、`{dd}` | 月、日，2 位 |
| `{000…}` | 流水號，3–8 個 `0`，位數不足時補零 |

- 日期代碼可以寫在同一個 `{}` 內（`{yyyyMM}`）；`{}` 以外的文字原樣保留；長度上限 50 字元。
- 必須**恰好一個**流水號。不認得的代碼、沒有或有多個流水號、單獨的 `{` `}`、控制字元 → 資料類別 API 回傳 `422 VALIDATION`（`details.field = numbering_pattern`）。空白視為不使用單號。
- `period_key` 由規則中的日期代碼依序組成（`SO{yyyyMM}-{0000}` → `202611`）；沒有日期代碼時為空字串，流水號永不重置。
- 日期以建立當下的 Asia/Taipei 日期為準（D13）。
- 流水號超過位數時繼續遞增，不截斷：`{000}` 的第 1000 號為 `1000`。

**取號**：在新增資料的同一個 transaction 內執行
`INSERT … ON CONFLICT (entity_id, period_key) DO UPDATE SET current_value = current_value + 1 RETURNING current_value`。
沒有該期間的列時新增（從 1 開始），有則遞增並鎖定該列直到 transaction 結束，效果與 `SELECT … FOR UPDATE` 後再更新相同；並行新增時依序取號，不會重複。資料寫入失敗 rollback 時號碼一併還原（不跳號）。

**規則**：

- 資料類別有設定規則時，通用資料 API 新增資料自動寫入 `custom_records.record_no`。
- `record_no` 不可由客戶端指定或修改：request 中帶了一律**忽略**；`data` 內帶 `record_no` 鍵視為未定義的欄位（422）。`record_no` 是保留的欄位代碼。
- 規則可以在使用後修改，只影響之後新增的資料，既有單號不變；改回原規則時接續該期間原本的流水號。
- 已刪除資料的單號不重複使用（流水號只增不減；唯一索引包含已刪除的資料）。
- 修改規則後產生的單號與既有單號重複時（例如 `{yy}-{000}` 改成 `26-{000}`），新增失敗並回傳 `422 VALIDATION`，需要調整規則。
- 列表可依 `record_no` 排序（文字排序）與篩選（`eq`、`contains`），見 §10。
- 第一版自訂資料類別沒有作廢狀態；系統資料類別的單號在 M4 處理。

#### `attachments`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_key / record_id / field_key | | 附件屬於哪筆資料的哪個欄位 |
| file_name / mime_type / size_bytes | | |
| sha256 | TEXT | |
| storage_path | TEXT | 伺服器磁碟上的相對路徑 |
| ＋共通欄位 | | |

---

### 8.5 客製化模組

#### `entity_definitions`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| key | TEXT UNIQUE | 例如 `customer`、`repair_order`（建立後不可修改） |
| label | TEXT | 顯示名稱 |
| icon | TEXT | |
| kind | TEXT | `master`（主檔）/ `document`（單據）/ `detail`（明細子表） |
| is_system | BOOLEAN | |
| storage | TEXT | `table`（系統資料表）/ `custom_records` |
| table_name | TEXT | storage = table 時，對應的資料表 |
| parent_entity_id | UUID FK → self | 明細子表所屬的主表 |
| numbering_pattern | TEXT | 單號規則 |
| title_template | TEXT | 資料標題範本，例如 `{record_no} {name}`；規則見下方 |
| sort_order | INTEGER | 選單排序 |
| is_active | BOOLEAN | |
| ＋共通欄位 | | |

**資料標題**（`title_template`，解析與產生在 `hammer_core::title`）：關聯欄位、選擇器、操作紀錄用來表示「這筆資料叫什麼」的文字。通用資料 API 的列表與單筆回應都有 `title`。

- 範本中 `{}` 內可寫 `{record_no}`（單號）或本資料類別的欄位代碼 `{field_key}`；`{}` 以外的文字原樣保留。長度上限 200 字元，不可有單獨的 `{` `}` 或控制字元。
- 建立與修改資料類別時檢查：只能引用 `record_no` 或此資料類別已定義的欄位（含已停用）；新的資料類別還沒有欄位，只能引用 `{record_no}`。不合法 → `422 VALIDATION`（`details.field = title_template`）。空白視為沒有範本。
- 引用關聯欄位的欄位（例如 `{customer.name}`）目前不支援（422），等 M3 系統資料類別一起評估。
- 值依欄位型態格式化，與桌面程式列表的顯示相同，但數值不加千分位、不補小數位數（存值寫入時已補齊）：

  | 型態 | 標題中的文字 |
  |---|---|
  | `text`、`long_text` | 原樣；換行等控制字元換成空白 |
  | `integer`、`decimal`、`money` | 存值原樣 |
  | `date` | `YYYY-MM-DD` |
  | `datetime` | 台北時間 `YYYY-MM-DD HH:mm`（D13） |
  | `boolean` | `是` / `否` |
  | `select` | 選項名稱（選項已移除時為選項值） |
  | `multi_select` | 選項名稱，以 `、` 連接 |
  | `relation`、`file`、`formula` | 目前為空字串 |

- 空值、引用已停用的欄位、存值格式不正確 → 空字串；結果去除前後空白。範本產生的結果是空字串時就回傳空字串，不改用預設規則。
- 沒有設定範本時，依序使用第一個有值的：單號 → 排序最前面、有效的 `text` 欄位的值 → id 前 8 碼。資料庫中的範本不合法（加入檢查前存入的）時也使用預設規則。
- 標題在讀取時產生，不存入資料庫；第一版不能依 `title` 排序或篩選。

#### `field_definitions`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_id | UUID FK → entity_definitions | |
| field_key | TEXT | 建立後不可修改；UNIQUE (entity_id, field_key) |
| label | TEXT | |
| data_type | TEXT | 見 7.3 |
| is_system | BOOLEAN | |
| storage | TEXT | `column` / `json` |
| column_name | TEXT | storage = column 時使用 |
| is_required / is_visible / is_readonly | BOOLEAN | |
| is_searchable / is_unique | BOOLEAN | 會觸發建立索引（7.5） |
| default_value | JSONB | |
| validation | JSONB | 例如 `{"min":"0","max":"100","max_length":50,"pattern":"^\\d{8}$"}`；`min`、`max` 與 decimal 值一樣存成字串（D9） |
| options | JSONB | select 的選項：`[{"value":"a","label":"A","color":"blue"}]` |
| decimal_places | SMALLINT | decimal / money 使用 |
| relation_entity_id | UUID FK → entity_definitions | relation 使用 |
| formula | TEXT | formula 使用 |
| formula_result_type | TEXT | formula 的結果型態 |
| formula_deps | JSONB | 解析後的相依欄位，供相依圖使用 |
| ui_widget | TEXT | 選用的輸入元件 |
| column_width | INTEGER | 列表欄寬 |
| help_text | TEXT | |
| sort_order | INTEGER | |
| is_active | BOOLEAN | |
| ＋共通欄位 | | |

#### `schema_change_logs`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| changed_at / changed_by | | |
| target_type | TEXT | `entity` / `field` |
| target_id | UUID | |
| action | TEXT | `create` / `update` / `deactivate` |
| before_data / after_data | JSONB | |

#### `view_layouts`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_id | UUID FK | |
| user_id | UUID FK，可為空 | 空值代表全域版面（需要 `schema.manage`）；個人版面只有本人可以修改 |
| name | TEXT | |
| config | JSONB | 顯示欄位與順序、欄寬、排序、篩選條件（排序與篩選格式與通用資料 API 相同，見 §10） |
| is_default | BOOLEAN | |
| ＋共通欄位 | | |

索引：`(entity_id, user_id)`；部分唯一索引保證每個使用者在同一個資料類別最多一個預設版面、全域預設也只有一個（不含已刪除）。設定新的預設時，同一範圍原本的預設在同一個 transaction 內取消。

- 預設版面的取得順序：使用者自己的預設 → 全域預設 → 依欄位定義自動產生（有效且顯示的欄位，依欄位排序）。
- `config` 參照到已停用或不存在的欄位（或欄位型態變更後已不適用的排序、篩選條件）時，讀取時略過，不回傳錯誤；寫入時則必須合法。
- 型別與端點見 `hammer_shared::layout`。

#### `custom_records`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_id | UUID FK → entity_definitions | |
| parent_record_id | UUID FK → self | 明細子表指向主表的資料 |
| record_no | TEXT | 依單號規則產生（如果有設定），見 §8.4 `number_sequences` |
| data | JSONB | 所有欄位的值，鍵為 field_key |
| ＋共通欄位 | | |

索引：`(entity_id)`、`(parent_record_id)`、`GIN(data)`、部分唯一索引 `(entity_id, record_no) WHERE record_no IS NOT NULL`（含已刪除的資料），以及第 7.5 節所述的動態索引。

---

### 8.6 基本資料

#### `units`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| code | TEXT UNIQUE | 例如 `pcs`、`box` |
| name | TEXT | 個、箱 |
| ＋共通欄位 | | |

#### `product_categories`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| parent_id | UUID FK → self | 支援多層分類 |
| name | TEXT | |
| sort_order | INTEGER | |
| ＋共通欄位 | | |

#### `products`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| code | TEXT UNIQUE | 料號 |
| name | TEXT | |
| spec | TEXT | 規格 |
| product_type | TEXT | `goods`（實體商品，有批號和庫存）/ `service`（服務，無庫存） |
| category_id | UUID FK | |
| unit_id | UUID FK | |
| barcode | TEXT | |
| tax_type | TEXT | `taxable`（應稅）/ `zero_rated`（零稅率）/ `exempt`（免稅） |
| default_sale_price | NUMERIC(18,4) | 台幣 |
| safety_stock | NUMERIC(18,4) | 安全庫存 |
| is_active | BOOLEAN | |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `currencies`
| 欄位 | 型態 | 說明 |
|---|---|---|
| code | TEXT PK | ISO 4217，例如 `TWD`、`USD`、`JPY` |
| name | TEXT | |
| symbol | TEXT | |
| decimal_places | SMALLINT | |
| is_base | BOOLEAN | 只有 TWD 為 true |
| is_active | BOOLEAN | |

#### `exchange_rates`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| currency_code | TEXT FK → currencies | |
| rate_date | DATE | |
| rate | NUMERIC(18,6) | 1 外幣 = ? 台幣 |
| | | UNIQUE (currency_code, rate_date) |
| ＋共通欄位 | | |

> 單據會帶入當天（或最近一天）的匯率作為預設值，使用者可以修改。單據上存的是實際使用的匯率，之後修改匯率表不會影響已建立的單據。

#### `tax_rates`
| 欄位 | 型態 | 說明 |
|---|---|---|
| code | TEXT PK | `taxable` / `zero_rated` / `exempt` |
| name | TEXT | |
| rate | NUMERIC(6,4) | 0.0500 / 0 / 0 |

---

### 8.7 客戶與廠商

#### `customers`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| code | TEXT UNIQUE | 客戶編號 |
| name | TEXT | 公司名稱 |
| short_name | TEXT | 簡稱 |
| tax_id | TEXT | 統一編號 |
| phone / fax / email | TEXT | |
| address | TEXT | 聯絡地址 |
| invoice_title | TEXT | 發票抬頭 |
| invoice_address | TEXT | 發票地址 |
| payment_terms_days | INTEGER | 付款條件（月結天數） |
| credit_limit | NUMERIC(18,2) | 信用額度 |
| is_active | BOOLEAN | |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `customer_contacts`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| customer_id | UUID FK | |
| name / title / phone / mobile / email | TEXT | |
| is_primary | BOOLEAN | 主要聯絡人 |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `suppliers`
欄位與 `customers` 相同，另外加上：

| 欄位 | 型態 | 說明 |
|---|---|---|
| default_currency_code | TEXT FK → currencies | 預設交易幣別 |
| country | TEXT | |
| bank_info | TEXT | 匯款資訊 |

#### `supplier_contacts`
欄位與 `customer_contacts` 相同（`customer_id` 改為 `supplier_id`）。

---

### 8.8 進貨模組

#### `purchase_orders`（採購單）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| order_no | TEXT UNIQUE | |
| supplier_id | UUID FK | |
| order_date | DATE | |
| expected_date | DATE | 預計到貨日 |
| currency_code | TEXT FK | |
| exchange_rate | NUMERIC(18,6) | |
| status | TEXT | `draft` / `confirmed` / `partially_received` / `closed` / `void` |
| subtotal / tax_amount / total | NUMERIC(18,4) | 外幣（單據幣別）金額 |
| total_twd | NUMERIC(18,2) | 換算台幣（參考用） |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `purchase_order_lines`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| purchase_order_id | UUID FK | |
| line_no | INTEGER | |
| product_id | UUID FK | |
| qty | NUMERIC(18,4) | |
| unit_price | NUMERIC(18,4) | 單據幣別 |
| amount | NUMERIC(18,4) | |
| received_qty | NUMERIC(18,4) | 已進貨數量（進貨過帳時累加） |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `goods_receipts`（進貨單）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| receipt_no | TEXT UNIQUE | |
| supplier_id | UUID FK | |
| purchase_order_id | UUID FK，可為空 | 可以不經採購單直接進貨 |
| receipt_date | DATE | |
| currency_code | TEXT FK | |
| exchange_rate | NUMERIC(18,6) | |
| status | TEXT | `draft` / `posted` / `void` |
| subtotal / tax_amount / total | NUMERIC(18,4) | 單據幣別 |
| total_twd | NUMERIC(18,2) | |
| posted_at / posted_by | | |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `goods_receipt_lines`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| goods_receipt_id | UUID FK | |
| line_no | INTEGER | |
| purchase_order_line_id | UUID FK，可為空 | |
| product_id | UUID FK | |
| qty | NUMERIC(18,4) | |
| unit_price | NUMERIC(18,4) | 單據幣別 |
| amount | NUMERIC(18,4) | |
| unit_cost_twd | NUMERIC(18,4) | = unit_price × exchange_rate（⏳ 進口成本分攤待討論） |
| lot_no | TEXT | 使用者輸入；留空時依規則自動產生 |
| lot_id | UUID FK → inventory_lots | 過帳後回填 |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `purchase_returns` / `purchase_return_lines`（進貨退回）
- 表頭欄位比照 `goods_receipts`（`receipt_no` 改為 `return_no`，並加上 `goods_receipt_id`）。
- 明細必須指定 `lot_id`，因為是退回特定批號。過帳時透過 `lot_allocations` 扣減該批號的庫存。

---

### 8.9 銷貨模組

> 銷貨以台幣交易，不設幣別欄位。

#### `sales_orders`（訂單）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| order_no | TEXT UNIQUE | |
| customer_id | UUID FK | |
| order_date | DATE | |
| delivery_date | DATE | 預計交貨日 |
| status | TEXT | `draft` / `confirmed` / `partially_shipped` / `closed` / `void` |
| tax_included | BOOLEAN | 單價是否含稅 |
| subtotal / tax_amount / total | NUMERIC(18,2) | |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `sales_order_lines`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| sales_order_id | UUID FK | |
| line_no | INTEGER | |
| product_id | UUID FK | |
| qty | NUMERIC(18,4) | |
| unit_price | NUMERIC(18,4) | |
| discount_rate | NUMERIC(6,4) | |
| amount | NUMERIC(18,2) | |
| shipped_qty | NUMERIC(18,4) | 已出貨數量 |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `shipments`（出貨單）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| shipment_no | TEXT UNIQUE | |
| customer_id | UUID FK | |
| sales_order_id | UUID FK，可為空 | |
| ship_date | DATE | |
| status | TEXT | `draft` / `posted` / `void` |
| tax_included | BOOLEAN | |
| subtotal / tax_amount / total | NUMERIC(18,2) | |
| cost_total_twd | NUMERIC(18,2) | 銷貨成本合計（過帳時計算） |
| ship_address | TEXT | |
| posted_at / posted_by | | |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `shipment_lines`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| shipment_id | UUID FK | |
| line_no | INTEGER | |
| sales_order_line_id | UUID FK，可為空 | |
| product_id | UUID FK | |
| qty | NUMERIC(18,4) | |
| unit_price | NUMERIC(18,4) | |
| amount | NUMERIC(18,2) | |
| lot_selection_mode | TEXT | `fifo` / `manual` |
| cost_amount_twd | NUMERIC(18,2) | 過帳時，依 lot_allocations 加總 |
| notes | TEXT | |
| custom_data | JSONB | |
| ＋共通欄位 | | |

#### `sales_returns` / `sales_return_lines`（銷貨退回）
- 表頭比照 `shipments`，並加上 `shipment_id`。
- 明細須對應原本的 `shipment_line_id`。過帳時依原出貨的 `lot_allocations`，把數量加回原批號，成本也用原批號的成本。

---

### 8.10 庫存模組

#### `inventory_lots`（批號）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| product_id | UUID FK | |
| lot_no | TEXT | UNIQUE (product_id, lot_no) |
| received_at | TIMESTAMPTZ | 先進先出的排序依據 |
| source_type | TEXT | `goods_receipt` / `adjustment`（期初或盤盈） |
| source_line_id | UUID | |
| qty_in | NUMERIC(18,4) | 原始入庫數量 |
| qty_remaining | NUMERIC(18,4) | CHECK (qty_remaining >= 0) |
| unit_cost_twd | NUMERIC(18,4) | |
| ＋共通欄位 | | |

索引：`(product_id, received_at) WHERE qty_remaining > 0`，供先進先出查詢使用。

#### `lot_allocations`（批號扣除明細）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| source_type | TEXT | `shipment_line` / `purchase_return_line` / `adjustment_line` |
| source_line_id | UUID | |
| lot_id | UUID FK → inventory_lots | |
| qty | NUMERIC(18,4) | |
| unit_cost_twd | NUMERIC(18,4) | 扣除當下該批號的成本 |
| created_at | TIMESTAMPTZ | |

#### `inventory_transactions`（庫存異動流水帳）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| txn_at | TIMESTAMPTZ | |
| txn_type | TEXT | `receipt` / `shipment` / `sales_return` / `purchase_return` / `adjustment` / `void_reversal` |
| product_id | UUID FK | |
| lot_id | UUID FK | |
| qty_change | NUMERIC(18,4) | 入庫為正、出庫為負 |
| unit_cost_twd | NUMERIC(18,4) | |
| ref_doc_type / ref_doc_id / ref_line_id | | 來源單據 |
| created_by | UUID FK | |

> 流水帳只新增，不修改也不刪除。作廢單據時新增反向異動。

#### `stock_adjustments` / `stock_adjustment_lines`（盤點與調整單）
| 欄位（明細） | 型態 | 說明 |
|---|---|---|
| product_id | UUID FK | |
| lot_id | UUID FK，可為空 | 盤盈時可新增批號 |
| qty_before / qty_after | NUMERIC(18,4) | |
| unit_cost_twd | NUMERIC(18,4) | 新批號的成本 |
| reason | TEXT | |

表頭欄位：`adjustment_no`、`adjustment_date`、`status`、`notes`、`custom_data`，加上共通欄位。期初庫存也用這張單據輸入。

#### `v_stock_balances`（檢視表）
```sql
SELECT product_id, SUM(qty_remaining) AS qty_on_hand,
       SUM(qty_remaining * unit_cost_twd) AS stock_value_twd
FROM inventory_lots
WHERE deleted_at IS NULL
GROUP BY product_id;
```

---

### 8.11 發票模組 ⏳ 待討論

先保留以下方向，細節等實作到這個模組時再定：
- `invoices` / `invoice_lines`：銷項與進項發票，只做內部紀錄
- 字軌號碼管理、作廢、折讓
- 與出貨單、進貨單的關聯

### 8.12 財務模組 ⏳ 待討論

先保留以下方向：
- 應收、應付帳款，以及收付款沖帳
- 外幣應付的匯兌損益
- 是否要做完整的複式記帳（會計科目、傳票、總帳）

---

## 9. 核心業務流程

### 9.1 進貨過帳

```
1. 鎖定進貨單（version 檢查）
2. 每一行明細：
   a. unit_cost_twd = unit_price × exchange_rate
   b. 建立 inventory_lots（lot_no 留空時自動產生）
   c. 新增 inventory_transactions（+qty）
   d. 如果有對應的採購單明細：累加 received_qty，並更新採購單狀態
3. status = posted
4. 寫入 audit_logs
（以上全部在同一個資料庫交易內完成）
```

### 9.2 出貨過帳（先進先出或手動指定批號）

```
1. 鎖定出貨單
2. 每一行明細：
   a. 用 SELECT … FOR UPDATE 鎖定該商品可用的批號
   b. 先進先出模式：依 received_at 由舊到新扣，直到扣滿數量
      手動模式：依使用者指定的批號與數量，逐一檢查 qty ≤ qty_remaining
   c. 庫存不足 → 整張單據失敗、回復原狀，並告知是哪個商品不足多少
   d. 建立 lot_allocations、扣減 qty_remaining
   e. 新增 inventory_transactions（−qty）
   f. cost_amount_twd = Σ(allocation.qty × allocation.unit_cost_twd)
3. 累加訂單的 shipped_qty 並更新訂單狀態
4. status = posted；計算 cost_total_twd
5. 寫入 audit_logs
```

出貨單草稿階段，介面會依先進先出顯示「建議批號」，使用者可以切換成手動並修改。

### 9.3 作廢規則

| 單據 | 作廢條件 | 作廢動作 |
|---|---|---|
| 出貨單 | 隨時（⏳ 已開發票時的限制，等發票模組再定） | 依 lot_allocations 把數量加回原批號，新增反向異動 |
| 進貨單 | 該批號沒有被扣過（qty_remaining = qty_in） | 刪除批號（軟刪除），新增反向異動；已被扣用時拒絕，提示先處理後續單據 |
| 退貨單 | 隨時 | 反向處理 |

### 9.4 樂觀鎖定

- 更新時使用 `UPDATE … SET …, version = version + 1 WHERE id = $1 AND version = $2`。
- 影響筆數為 0 時，回傳 `409 Conflict`。介面提示「此資料已被其他人修改，請重新載入」。

---

## 10. API 設計原則

- REST + JSON，路徑前綴 `/api/v1`。
- 認證：`Authorization: Bearer <session token>`。
- 客戶端在 `X-Client-Version` 標頭帶上版本號。版本低於伺服器要求時回傳 `426`，介面提示更新。
- 系統資料類別有各自的端點（例如 `/api/v1/shipments/{id}/post`）。
- 自訂資料類別共用通用端點：`/api/v1/entities/{entity_key}/records`。
- 錯誤格式統一：
  ```json
  { "code": "STOCK_INSUFFICIENT", "message": "商品 A001 庫存不足，尚缺 5", "details": { … } }
  ```
- 分頁：資料量會持續增加的列表 API（操作紀錄、單據、業務資料等）一律分頁。型別與常數在 `hammer_shared::pagination`。
  - 參數（query string）：`page` 從 1 開始，預設 1；`page_size` 預設 50、上限 200，超過上限時套用上限（不是錯誤）。小於 1 時回傳 `422 VALIDATION`，`details.field` 為參數名稱；不是整數時回傳 `400 BAD_REQUEST`。
  - 回應：
    ```json
    { "items": [ … ], "total": 128, "page": 1, "page_size": 50 }
    ```
    `total` 是符合篩選條件的總筆數；`page`、`page_size` 是實際套用的值。頁碼超過最後一頁時 `items` 為空陣列。
  - 排序必須是確定的：主要排序欄位之後加上 `id` 作為次要排序（uuid v7 依時間遞增），避免同值的資料在頁與頁之間重複或遺漏。
  - 篩選條件與分頁參數放在同一個 query 型別，不使用 `#[serde(flatten)]`（query string 搭配 flatten 時數字欄位無法解析）。空字串的文字條件視為未指定；時間使用 RFC 3339，區間為「含開始、不含結束」。
  - 筆數很少、不會持續增加的列表（角色、帳號、幣別等）可以不分頁，直接回傳陣列。
- 業務資料列表的篩選與排序（通用資料 API 先採用，之後系統資料類別的列表與 `view_layouts` 的篩選條件沿用同一格式；型別在 `hammer_shared::record`）：
  - 排序：`sort=<field_key>` 由小到大，`sort=-<field_key>` 由大到小；省略時依建立順序由新到舊（`id DESC`）。只能使用有效（未停用）的欄位。依欄位型態比較（`integer`、`decimal`、`money` 以數值，`date`、`datetime` 以時間），不會按字串排序；沒有值的資料排在最後。
  - 篩選：`filter.<field_key>.<op>=<value>`，例如 `filter.qty.gte=10`。多個條件須全部符合（AND）。
    | op | 說明 | 適用型態 |
    |---|---|---|
    | `eq` | 等於 | `multi_select` 以外 |
    | `contains` | 包含（不分大小寫） | `text`、`long_text` |
    | `gt`、`gte`、`lt`、`lte` | 大於、大於等於、小於、小於等於 | `integer`、`decimal`、`money`、`date`、`datetime` |
    | `empty` | `true`：沒有值；`false`：有值 | 全部 |
    | `any` | 包含任一選項；可重複帶多個（`filter.tags.any=a&filter.tags.any=b`），符合任一個即可 | `multi_select` |
  - 值的格式：數值 `12.5`、日期 `YYYY-MM-DD`、日期時間 RFC 3339、勾選 `true` / `false`。值為空字串的條件視為未指定。
  - 欄位不存在或已停用、運算子不適用於該型態、值的格式錯誤 → `422 VALIDATION`，`details.field` 為參數名稱（例如 `filter.qty.gte`）。不認得的參數名稱忽略。
  - 單號：`sort` 與 `filter` 可使用 `record_no`（保留的代碼，不是欄位），依文字排序，篩選只支援 `eq`、`contains`（§8.4）。

---

## 11. 維運

| 項目 | 做法 |
|---|---|
| 開發環境 | `deploy/docker-compose.yml` 啟動 PostgreSQL；API Server 用 `cargo run` 啟動 |
| 資料庫版本 | sqlx migrations。伺服器啟動時自動套用 |
| 初始資料 | migration 內建三種角色與權限、內建資料類別與欄位定義、幣別、稅率。第一次啟動時引導建立管理者帳號 |
| 備份 | 每日 `pg_dump`，保留 30 天；並提供管理者手動備份與還原的功能（⏳ 備份存放位置待定） |
| 附件 | 存在伺服器磁碟，與資料庫一起備份 |
| CI | GitHub Actions：在 macOS 和 Windows 建置桌面程式；執行 `cargo test`、`clippy`、前端 lint |
| 自動更新 | Tauri updater 外掛 |
| 程式碼簽章 | ⏳ 正式發佈前處理（macOS 需要 Apple Developer 帳號） |

---

## 12. 開發階段

| 階段 | 內容 |
|---|---|
| M0 | Workspace 骨架、Docker（PostgreSQL）、CI、資料庫 migration |
| M1 | API Server 基礎、登入、三種角色權限、操作紀錄、Tauri 登入畫面 |
| M2a | 欄位定義引擎，以及依定義自動產生的列表與表單；`custom_records` 與通用資料 API，管理者可建立最基本的自訂資料類別（只支援 `master`） |
| M2b | 明細子表、關聯欄位、附件欄位、單號規則，以及資料類別管理的完整功能 |
| M2c | 公式引擎 |
| M3 | 客戶、廠商、商品、單位、幣別、匯率等基本資料 |
| M4 | 採購、進貨（含外幣）、批號、訂單、出貨（先進先出或手動指定）、退貨、盤點 |
| M5 | 發票（⏳） |
| M6 | 財務（⏳） |
| M7 | 報表、Excel 匯出、列印、備份與還原介面 |

---

## 13. 待決事項

| # | 項目 | 何時決定 |
|---|---|---|
| Q1 | 發票模組細節（字軌、作廢、折讓、與出貨的關聯） | M5 開始前 |
| Q2 | 財務模組深度（是否做複式記帳、匯兌損益） | M6 開始前 |
| Q3 | 進口成本分攤（運費、關稅、報關費）到批號成本 | M4 進貨功能開始前 |
| Q4 | Tailscale 方案、伺服器主機、備份存放位置 | 開放多人使用前 |
| Q5 | 可編輯者能不能維護匯率 | M3 開始前 |
| Q6 | 程式碼簽章 | 正式發佈前 |
