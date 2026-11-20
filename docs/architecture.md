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
| 發票管理 | 銷項發票、銷貨折讓、進項憑證、進貨折讓的內部紀錄（手開號碼，不上傳財政部，§8.11） |
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
| `settings.manage` | 系統設定（幣別等） | ✅ | ❌ | ❌ |
| `audit.read` | 查看操作紀錄 | ✅ | ❌ | ❌ |

> 幣別與匯率（Q5 已決定，規則見 §8.6 `currencies`、`exchange_rates`）：
>
> - 匯率的新增、修改、刪除需要 `record.write`（可編輯者可以），查看需要 `record.read`。匯率是日常輸入的業務資料，不是系統設定。
> - 幣別的新增、修改、停用需要 `settings.manage`（只有管理者），查看只需要登入。
> - 單號規則屬於資料類別設定，需要 `schema.manage`（§8.5）。

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
| 例子 | 客戶、商品（M3）；訂單、進貨單（M4） | 管理者自建的「維修單」 |
| `entity_definitions` | `is_system = true`、`storage = table`，由 migration 建立 | `is_system = false`、`storage = custom_records` |
| 儲存 | 專屬資料表：系統欄位是實體欄位（`storage = column`），自訂欄位存在 `custom_data` JSONB | 通用資料表 `custom_records.data` JSONB |
| 資料 API 與畫面 | 通用資料 API（`/api/v1/entities/{key}/records`）與桌面的動態列表、表單；系統單據的過帳、作廢另有專屬端點（下方「系統單據」、§10「單據動作」） | 通用資料 API 與動態列表、表單 |
| 可否刪除資料類別 | 不可，也不可停用 | 可（軟刪除） |
| 影響庫存與財務 | 依系統邏輯 | 第一版不影響（D8） |

**系統資料類別共用通用資料 API**（M3 起）：

- 列表、單筆、新增、修改、刪除、選擇器（`lookup`）、篩選與排序、`view_layouts`、操作紀錄都與自訂資料類別相同；權限一樣是 `record.read` / `record.write`。差別只在 `hammer_core` 依 `storage` 把資料讀寫到專屬資料表。
- request 與回應的 `data` 以 `field_key` 為鍵，同時包含系統欄位與自訂欄位，客戶端不需要區分；伺服器把系統欄位寫入實體欄位，其餘寫入 `custom_data`。值的格式一律依 §7.3（例如 `NUMERIC` 欄位讀出時是補齊 `decimal_places` 的字串）。
- 系統欄位的 `field_key` 與 `column_name` 相同（例如 `unit_id`）。外鍵也是欄位（`relation`，§7.3）；`id`、明細的父資料欄位（`customer_id` / `supplier_id`）與 `sort_order`、共通欄位、`custom_data`、`formula_errors` 不是欄位；request 帶到時與其他未定義的欄位相同（`422`）。
- 主檔類的系統資料類別（M3）沒有單號：`record_no` 為 `null`，`numbering_pattern` 不可設定（`422`，`details.field = numbering_pattern`）；系統單據的單號見下方「系統單據」。
- 系統明細子表（`customer_contact`、`supplier_contact`）與自訂明細子表的規則相同（§8.5 `custom_records`「明細子表」、§10「主表與明細」）：只能透過主表存取，以 `lines.<明細子表代碼>` 整組取代。順序存在明細資料表的 `sort_order`，父資料存在 `customer_id` / `supplier_id`。
- 管理者可以在系統資料類別（含系統明細子表）新增自訂欄位（含公式、關聯、附件欄位），規則與自訂資料類別相同；不可新增系統欄位，也不可在系統資料類別下建立新的明細子表。
- 公式欄位的計算結果存在 `custom_data`，錯誤存在該資料表的 `formula_errors`（§7.6「存檔時計算」）；公式可以引用系統欄位（依其 `data_type` 讀取）。
- 系統資料表的 `is_active` 欄位（商品、客戶、廠商）是一般的 `boolean` 系統欄位：選擇器（`lookup`）不列出 `is_active = false` 的資料；寫入關聯欄位時不檢查，既有資料仍可指向已停用的資料。

**系統單據**（M4 起；`kind = document` 的系統資料類別，例如 `stock_adjustment`、M4b 的採購單、進貨單、進貨退回（§8.8）、M4c 的訂單、出貨單、銷貨退回（§8.9））：除了上述規則，另外依照以下規則。邏輯在 `hammer_core::documents`，型別在 `hammer_shared::document`。管理者自建的 `document` 資料類別（自訂資料類別）不適用，沒有狀態與過帳（§8.5）。

- **單號**：
  - 單據的主表一律有 `record_no TEXT NOT NULL` 欄位，取代各單據自己的單號欄位（`order_no`、`receipt_no`、`shipment_no`、`adjustment_no` 等都不再使用）；唯一索引 `(record_no)` 包含已刪除的資料，名稱一律為 `uq_<資料表>_record_no`（例如 `uq_stock_adjustments_record_no`；伺服器依名稱辨識單號重複）。單據明細沒有單號。
  - 新增時依 `entity_definitions.numbering_pattern` 取號，規則與自訂資料類別相同（§8.4「單號規則」「取號」「規則」）：以建立當下的 Asia/Taipei 日期產生（不是單據日期）、不可由客戶端指定或修改、已刪除資料的單號不重複使用、規則修改後與既有單號重複時新增失敗（`422`）。
  - 每張單據的預設單號規則由 migration 寫入（盤點調整單為 `ADJ{yyyyMM}-{0000}`，M4b 的單據見 §8.8，M4c 的單據見 §8.9）。管理者可以修改（`PATCH /api/v1/entities/{key}`，需要 `schema.manage`），**不可清空**（`422`，`details.field = numbering_pattern`）。主檔類的系統資料類別（M3）仍不可設定單號規則。
  - 回應的 `record_no`、標題範本的 `{record_no}`、列表依 `record_no` 排序與篩選（§10）都與自訂資料類別相同。
- **系統維護的欄位**：由伺服器寫入，使用者不能輸入。
  - `status`（狀態）、金額合計（例如 M4b、M4c 的 `subtotal`、`tax_amount`、`total`），以及各單據定義為「系統維護」的欄位（例如採購明細的 `received_qty`、進貨明細的 `unit_cost_twd`，§8.8）是系統欄位（`field_definitions` 有定義，`is_readonly = true`）：出現在 `data`，可以篩選、排序、被公式與標題範本引用。管理者可以修改顯示名稱等（§7.2），但不可設為必填、不可設定預設值（`422`，`details.field` 為該屬性）。
  - `posted_at`、`posted_by`、`voided_at`、`voided_by`，以及明細上過帳時寫入的 `lot_id` 等，**不是**欄位（與共通欄位相同）：不出現在 `data`；主表的這些值放在回應的 `document` 物件（`{ "posted_at", "posted_by", "voided_at", "voided_by", "closed_at", "closed_by" }`，非單據的資料為 `null`；`closed_at`、`closed_by` 只有採購單、訂單的手動結案會寫入，其他單據為 `null`；採購單、訂單的 `posted_at`、`posted_by` 是確認的時間與人員，§8.8、§8.9）。
  - request 的 `data` 帶了系統維護的欄位（`status`、金額合計）時**一律忽略**，不回傳錯誤（與公式欄位相同，§7.6）；帶了不是欄位的鍵（`posted_at`、`lot_id` 等）時與其他未定義的欄位相同（`422`）。
  - 金額合計在每次存檔時由伺服器依明細重新計算（`hammer_core`，不使用公式引擎）；過帳時寫入的值（例如成本）在過帳的 transaction 內寫入。新增時 `status = draft`。
- **單據明細**是系統明細子表（`kind = detail`、`storage = table`），規則與 §7.1 上方的系統明細子表相同：只能透過主表存取，以 `lines.<明細子表代碼>` 整組取代；順序存在 `sort_order`（取代原本設計的 `line_no`），父資料欄位 `<主表單數>_id`（例如 `stock_adjustment_id`）不是欄位。明細的 id 在單據過帳後就固定（批號、批號扣除、異動都以明細 id 指向來源）。
- **狀態限制**（狀態流程見 §8.2）：
  - 只有 `draft` 可以修改、刪除（通用資料 API 的 `PATCH`、`DELETE`）。其他狀態 → `422 INVALID_STATUS`，`details` 為 `{ "status": "posted" }`，訊息「已過帳的單據不可修改」「已作廢的單據不可刪除」等。主表、明細、自訂欄位都不可修改。
  - 檢查順序：權限 → 資料存在（`404`）→ `version`（`409 CONFLICT`）→ 狀態（`422 INVALID_STATUS`）→ 內容驗證（`422 VALIDATION`）。
  - 刪除草稿與其他資料相同（軟刪除，明細一起刪除，單號不再使用）。
  - 過帳、作廢使用專屬端點（§10「單據動作」），需要 `record.post`。
  - 「重新計算」（§7.6）不受狀態限制：已過帳、已作廢的單據的公式欄位也會重算（公式只寫入 `custom_data`，不影響庫存與金額）。
- **操作紀錄**：新增、修改、刪除與其他資料相同；過帳、作廢的 `action` 為 `post`、`void`，diff 記錄 `status` 與過帳時寫入的欄位（含明細，鍵為 `lines.<明細子表代碼>`）；`posted_at` 等不是欄位的值不記錄（`audit_logs` 本身有時間與操作者）。

### 7.2 欄位的兩個層級

| | 系統欄位（`is_system = true`） | 自訂欄位 |
|---|---|---|
| 儲存位置 | 資料表的實體欄位 | `custom_data` / `data` JSONB |
| 可修改 | 顯示名稱、顯示或隱藏、排序、欄寬、說明文字；必填與預設值在不違反系統規則下可改 | 全部屬性 |
| 不可修改 | 資料型態、欄位代碼，也不可刪除 | 欄位代碼（建立後就固定） |
| 刪除 | 不可 | 只能停用（`is_active = false`），資料保留 |

**系統欄位的修改**（`hammer_core::schema::rules` 的 `plan_update_field`；欄位定義由 migration 寫入，清單見 §8.6、§8.7）：

| 屬性 | 可否修改 |
|---|---|
| `label`、`is_visible`、`sort_order`、`column_width`、`help_text` | 可以 |
| `is_required` | 可以設為必填；migration 定義為必填的不可取消（`422`，`details.field = is_required`） |
| `default_value` | 可以，必須符合欄位型態與該欄位的系統限制；`relation` 欄位不可設定（與自訂欄位相同） |
| `field_key`、`data_type`、`column_name`、`storage` | 不可 |
| `is_readonly`、`is_searchable`、`is_unique`、`validation`、`options`、`decimal_places`、`ui_widget`、`relation_entity_id`、`formula`、`formula_result_type` | 不可（`422`，`details.field` 為該屬性） |
| `is_active` | 不可停用 |

- 系統欄位的 `is_searchable`、`is_unique` 反映 migration 建立的靜態索引，不會觸發 §7.5 的動態索引。
- 新增資料時沒有帶值的欄位套用 `default_value`（與自訂欄位相同）；修改資料時不套用。

**各系統欄位的系統限制**（伺服器端檢查，違反時 `422 VALIDATION`，`details.fields` 指出欄位）：

| 欄位 | 限制 |
|---|---|
| `code`（單位、商品、客戶、廠商） | 一律必填；去除前後空白後儲存，不可有控制字元，最長 50 字元；**不分大小寫**不可重複（`A001` 與 `a001` 視為重複，儲存時保留輸入的大小寫），只比對未刪除的資料，刪除後代碼可以再使用。建立後可以修改（關聯以 id 指向資料，不受影響）。重複時訊息與「不可重複」的自訂欄位相同 |
| `name`（所有系統資料類別） | 一律必填；去除前後空白後不可為空 |
| `unit_id`（商品） | 一律必填；指向未刪除的單位 |
| `category_id`（商品） | 指向未刪除的商品分類 |
| `parent_id`（商品分類） | 指向未刪除的商品分類；不可指向自己或自己的下層分類（會形成循環） |
| `product_type`、`tax_type` | 一律必填；選項固定（§8.6），不可新增、修改或移除選項 |
| `default_currency_code`（廠商） | 一律必填，預設 `TWD`；值必須是有效（`is_active`）的幣別代碼。修改資料時只在值有變動時檢查，所以幣別停用後既有資料可以保留原值 |
| `tax_id`（客戶、廠商） | 有填時必須是 8 位數字（ASCII `0`–`9`）；不驗證檢查碼。可以不填 |
| `payment_terms_days` | 0 以上的整數 |
| `credit_limit`、`default_sale_price`、`safety_stock` | 0 以上 |
| `is_primary`（聯絡人） | 同一個客戶（廠商）最多一位主要聯絡人：存檔時檢查整組明細中 `is_primary = true` 最多一列（`422`，`details.lines` 指出第二列） |
| `is_active` | 必填，預設 `true` |

上表中「一律必填」「0 以上」等限制由 migration 寫入欄位定義（`is_required`、`validation`），因為這些屬性不可修改，所以限制固定；`hammer_core::master_data` 另外再檢查一次，不依賴欄位定義的內容。`code` 的不分大小寫唯一、`tax_id` 的格式、`parent_id` 的循環、`default_currency_code` 的幣別檢查、`is_primary` 的檢查另外寫在 `hammer_core`。修改資料時這些規則只檢查值有變動的欄位。

**刪除限制**（`hammer_core::master_data`，伺服器端檢查）：

| 資料類別 | 不能刪除的情況 |
|---|---|
| 單位 | 還有未刪除的商品使用（`unit_id`） |
| 商品分類 | 還有未刪除的商品使用（`category_id`），或還有未刪除的子分類（`parent_id`） |

- 違反時 `422 VALIDATION`，`details` 為 `{ "fields": [], "in_use_count": 3, "child_count": 1 }`（只列出大於 0 的數量），訊息例如「此單位還有 3 筆商品使用中，無法刪除」。
- 新增、修改商品或分類時以 `FOR SHARE` 鎖定所指的單位、分類（關聯檢查），刪除時先 `FOR UPDATE` 鎖定要刪除的資料再計數，所以計數後不會再有新的使用者。
- 廠商被採購、進貨單據引用時的限制見 §8.8「廠商、商品被進貨單據引用時的限制」；客戶被銷貨單據引用時的限制見 §8.9「客戶、商品被銷貨單據引用時的限制」。

**商品被庫存引用時的限制**（M4a 起，`hammer_core::master_data`；「有庫存異動」= `inventory_transactions` 中有該商品的資料，不論來源單據之後是否作廢）：

| 操作 | 規則 |
|---|---|
| 刪除商品 | 有庫存異動時不可刪除：`422 VALIDATION`，`details` 為 `{ "fields": [], "transaction_count": 12 }`，訊息「此商品已有庫存異動，無法刪除，請改為停用」 |
| `product_type`：`goods` → `service` | 有庫存異動時不可修改：`422 VALIDATION`，`details.fields` 指出 `product_type`，訊息「此商品已有庫存異動，無法改為服務」 |
| `product_type`：`service` → `goods` | 可以（服務沒有庫存異動） |
| `unit_id` | 有庫存異動時不可修改（既有數量會變成不同單位）：`422 VALIDATION`，`details.fields` 指出 `unit_id` |

- 刪除與修改時先以 `FOR UPDATE` 鎖定商品再檢查；過帳以 `FOR SHARE` 鎖定商品（§8.10「鎖定順序」），兩者互斥。
- 只被**草稿**單據的明細引用（還沒有異動）時不限制：刪除商品後草稿的明細保留原值（與關聯欄位相同，§7.3），過帳時檢查失敗（「商品已刪除」）；改類型或單位後同樣在過帳時重新檢查。
- 採購單、訂單引用商品時的限制見 §8.8「廠商、商品被進貨單據引用時的限制」、§8.9「客戶、商品被銷貨單據引用時的限制」。

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
| `formula` | 公式（唯讀，由系統計算） | 依 `formula_result_type` 決定（與該型態相同） |

**關聯欄位（`relation`）**（檢查在 `hammer_core::schema::rules`、`hammer_core::relations`；API 格式見 `hammer_shared::record`）：

- 建立欄位時必須指定 `relation_entity_id`，建立後不可修改。目標只能是有效、`kind` 為 `master` 或 `document` 的資料類別（可以是自己），自訂與系統資料類別都可以（M3 起，例如指向 `product`、`customer`）；不可指向明細子表（自訂或系統的都不可以，例如 `customer_contact`）。唯一的例外是 migration 建立的系統欄位 `purchase_order_line_id`、`goods_receipt_line_id`（§8.8），指向系統明細子表 `purchase_order_line`、`goods_receipt_line`：存在檢查、鎖定、`relations` 的標題與其他關聯欄位相同；明細子表沒有選擇器，所以這兩個欄位預設隱藏，由桌面程式在帶入時寫入。關聯欄位不可設定預設值；可以開啟可搜尋、不可重複（§7.5）。
- 寫入：值必須是目標資料類別中**未刪除**的資料 id，否則 `422 VALIDATION`（`details.fields` 指出欄位）。修改資料時只檢查有變動的關聯欄位，並以 `FOR SHARE` 鎖定目標資料到 transaction 結束，避免同時被刪除。目標是系統資料類別時查詢並鎖定它的專屬資料表；目標的 `is_active = false` 不影響寫入（§7.1）。
- 系統資料表的外鍵（`unit_id`、`category_id`、`parent_id`）也是指向系統資料類別的 `relation` 系統欄位，寫入、回應、篩選的規則與此相同；另外有資料庫外鍵（`REFERENCES`）。
- 目標資料之後被刪除：允許刪除，關聯欄位保留原值。
- 回應：列表與單筆回應有 `relations: { "<field_key>": { "id", "title", "deleted" } }`，列出 `data` 中每個有值的關聯欄位；`title` 依目標資料類別的標題規則產生（§8.5），目標已刪除時 `deleted = true`、`title` 為刪除前的標題。
- 篩選只支援 `eq`（值為資料 id）、`empty`；不能排序（§10）。
- 選擇器：`GET /api/v1/entities/{key}/records/lookup?q=&limit=` 依標題比對（以空白分隔的每個詞都要出現，不分大小寫），回傳未刪除資料的 `id`、`title`，由新到舊（系統資料類別另外排除 `is_active = false` 的資料）；`limit` 預設 20、上限 50。需要 `record.read`。標題不存入資料庫，所以依 id 分批讀取資料、產生標題後比對。
- 不做：反向列表（「這台設備的所有維修單」）、多選關聯。

**附件欄位（`file`）**（上傳、下載、綁定與清理在 `hammer_core::attachments`；API 格式見 `hammer_shared::attachment`）：

- 先上傳 `POST /api/v1/attachments`（`multipart/form-data`，檔案放在 `file` 欄位；需要 `record.write`），回傳 `id`、`file_name`、`mime_type`、`size_bytes`、`sha256`。此時附件**未綁定**。
- 新增或修改資料時把附件 id 寫入 `file` 欄位：值必須是未刪除、未綁定的附件，否則 `422 VALIDATION`（`details.fields` 指出欄位）；同一個附件不可用在第二筆資料或第二個欄位。寫入資料的 transaction 內以 `FOR UPDATE` 鎖定附件並綁定（寫入 `attachments` 的 `entity_key`、`record_id`、`field_key`）。修改資料時只處理有變動的 `file` 欄位。
- 欄位值被換掉或清空：舊附件標記刪除（`deleted_at`），檔案保留，之後無法下載。資料被軟刪除：附件保留，仍可下載。
- 回應：列表與單筆回應有 `attachments: { "<field_key>": { "id", "file_name", "mime_type", "size_bytes", "sha256", "created_at" } }`，列出 `data` 中每個有值的 `file` 欄位所指的附件（供顯示檔名、大小）；附件已標記刪除時不列出（`hammer_core::attachments::attach_files`）。
- 下載 `GET /api/v1/attachments/{id}`（需要 `record.read`）：`Content-Disposition: attachment` 帶原始檔名（`filename*` 為 UTF-8，支援中文）；未綁定的附件也可以下載（存檔前預覽）。
- 不可設定預設值、不能變更型態（§7.4）。篩選只支援 `empty`；不能排序。資料標題中為空字串（§8.5）。
- 不做：一個欄位多個檔案、圖片縮圖、線上預覽。

**公式欄位（`formula`）**（設定檢查在 `hammer_core::schema::rules`，公式的檢查在 `hammer_core::schema::formula`；語法與計算見 §7.6）：

- 建立與修改欄位時必須提供 `formula` 與 `formula_result_type`；結果型態只能是 `text`、`long_text`、`integer`、`decimal`、`money`、`date`、`datetime`、`boolean`，`decimal` / `money` 需要 `decimal_places`。其他型態不可設定 `formula`、`formula_result_type`。
- 公式與結果型態可以修改；修改後既有資料**不會**自動重新計算（重新計算另有功能）。只修改其他屬性時不重新檢查公式；重新啟用已停用的公式欄位時重新完整檢查。
- 可以設為「可搜尋」；不能設為「不可重複」、預設值、必填（`422`，`details.field` 為該屬性）。
- 儲存時的檢查見 §7.6「定義公式時的檢查」；不合法 → `422 VALIDATION`，`details` 為 `{ "field": "formula", "span": { "start", "end" } }`（錯誤在公式中的字元位置，從 0 開始、不含 `end`）或 `{ "field": "formula", "cycle": ["a", "lines.b", "a"] }`（循環路徑）。
- 被有效公式引用的欄位不能停用（含停用 API 與修改 API 的 `is_active = false`）；停用明細子表、刪除資料類別時也檢查是否被其他資料類別的有效公式引用。→ `422 VALIDATION`，`details` 為 `{ "field": "formula", "formulas": [{ "entity_key", "entity_label", "field_key", "field_label" }] }`（依資料類別、欄位代碼排序）。「有效公式」是啟用中、所屬資料類別未刪除（含已停用的資料類別）的公式欄位。
- 值由伺服器在存檔時計算（§7.6「存檔時計算」）；request 帶公式欄位的值時一律忽略。篩選與排序依 `formula_result_type` 支援對應的運算子（§10）。

### 7.4 欄位型態變更規則

| 從 → 到 | 規則 |
|---|---|
| 任意型態 → `text` / `long_text` | 允許 |
| `text` ↔ `long_text` | 允許 |
| `integer` → `decimal` | 允許 |
| `select` → `multi_select` | 允許 |
| `relation` → `text` / `long_text` | 允許（值為資料 id 的文字，轉換後清除 `relation_entity_id`） |
| `relation` → 其他型態、其他型態 → `relation` | 不允許 |
| `file` → 任意型態、任意型態 → `file` | 不允許（附件綁定在欄位上，轉換後無法維持附件與欄位值的對應） |
| `formula` → 任意型態、任意型態 → `formula` | 不允許（公式的值由系統計算，與輸入值無法互轉）；公式與結果型態以修改欄位變更（§7.3） |
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
- **系統資料表的動態索引**（自訂欄位存在 `custom_data`）：
  - 可搜尋：`CREATE INDEX … ON <table_name> ((custom_data->>'field_key'))`。
  - 不可重複：`CREATE UNIQUE INDEX … ON <table_name> ((custom_data->>'field_key')) WHERE deleted_at IS NULL`。
  - 資料表只存一個資料類別，所以**不加** `entity_id` 條件；系統明細子表（聯絡人）與自訂明細子表相同，所有明細（不分主表）不可重複。
  - 名稱規則、前綴（`cfs` / `cfu`）與自訂資料類別相同，以資料類別代碼組成（例如 `cfs_customer_region_…`）；資料類別代碼不分自訂或系統都不可重複，所以名稱不會衝突。
  - `table_name` 只來自 migration 寫入的 `entity_definitions`，拼接 DDL 前與 `field_key` 一樣檢查只含 `a-z`、`0-9`、`_` 並加上引號。
  - 建立前檢查重複、停用時移除、重新啟用時再建立、與欄位定義修改在同一個交易內等規則都與自訂資料類別相同。
- 系統欄位（實體欄位）不使用動態索引：需要的索引（`code` 的不分大小寫唯一索引、外鍵索引等）由 migration 建立（§8.6、§8.7）。

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
| 重新計算 | 修改公式後（或關聯資料修改後），管理者可對該資料類別的全部資料執行「重新計算」（見下方「重新計算」） |
| 限制 | 公式只能讀資料，不能寫入其他欄位；系統欄位（例如單據金額、稅額）不能用公式覆蓋 |

**明細子表的公式**：只能引用同一筆明細的欄位與關聯引用；不能引用主表欄位，也不能使用子表彙總。主表的公式以子表彙總讀取明細（包含明細的公式欄位）。因此循環只可能發生在同一張表內，計算時明細的公式先於主表。

**子表彙總**：`SUM(明細子表代碼.欄位代碼)`，參數只能是一個「明細子表代碼.欄位代碼」，明細子表必須是本資料類別有效的明細子表；`SUM`、`AVG`、`MIN`、`MAX` 只能用於數字欄位（或結果為數字的公式欄位），`COUNT` 可用於任何支援的型態。

**定義公式時的檢查**（`hammer_core::schema::formula`，使用 `crates/formula`）：

1. 語法（`hammer_formula::parse`）
2. 引用：本筆欄位存在且有效；關聯引用的關聯欄位存在且有效、目標資料類別有效，目標欄位存在且有效；子表彙總的明細子表有效，欄位存在且有效
3. 型別檢查（`hammer_formula::check_formula`）：運算子與函式的參數型態、`IF` / `COALESCE` 的結果型態一致，結果型態能存成 `formula_result_type`
4. 循環引用：以整個資料類別（主表與有效的明細子表）所有有效公式欄位的 `formula_deps` 重建相依圖（`hammer_formula::calculation_order`），引用自己也算循環
5. 通過後寫入 `formula_deps`（格式見 §8.5 `field_definitions`）

被引用的資料類別（關聯目標、明細子表）在檢查時以 `FOR SHARE` 鎖定；停用欄位、明細子表時以 `FOR UPDATE` 鎖定所屬資料類別，所以兩者依序進行，檢查通過的公式不會引用到剛被停用的欄位。

已知限制（之後處理）：被引用欄位的型態變更、被引用公式的結果型態變更，不會重新檢查引用它的公式；計算時型態不符的結果為空值（見上方「錯誤」）。

**存檔時計算**（計算在 `hammer_formula::record`，與桌面端預覽共用；讀取關聯目標等資料庫工作在 `hammer_core::formulas`，在 `hammer_core::records` 的新增與修改中呼叫）：

- 新增與修改資料（含主表與明細一起存檔）時，在同一個 transaction 內計算整個資料類別（主表與有效的明細子表）所有**有效**的公式欄位，順序依相依圖（`hammer_formula::calculation_order`）：先算每一列明細的公式，再算主表的公式；主表的子表彙總讀取明細公式的結果。
- 修改時所有公式都重算（不只有變動的欄位）。資料類別有公式時，request 沒帶到的明細子表也一起重算（視為列出所有既有明細且不修改欄位），主表的彙總因此使用全部明細；明細的值有變動時一併寫入。重算後主表與明細都沒有變動時不寫入、`version` 不變。
- request 的 `data`（主表與明細）帶了公式欄位的值時一律忽略，不回傳錯誤（含已停用的公式欄位）。已停用的公式欄位不計算，既有的值保留。
- 公式引用的欄位依目前的欄位定義，只能讀取有效的欄位；`formula` 欄位以 `formula_result_type` 讀取。
- 關聯引用讀取目標資料**目前**的值（目標是系統資料類別時，系統欄位與 `custom_data` 都可以引用），目標已軟刪除時仍讀取其資料；關聯欄位沒有值時為空值。目標資料之後被修改，引用它的公式**不會**自動更新，要等該筆資料再次存檔或執行「重新計算」。
- `TODAY()` 為存檔時間在 Asia/Taipei 的日期（D13）。
- 結果依 `formula_result_type` 轉成 §7.3 的存值寫入 `data`；空值與空字串不寫入（與一般欄位相同，沒有值的欄位不出現在 `data`）。
- 計算失敗（除以零、型態不符、溢位、引用的欄位不存在等）時不中斷存檔：該欄位沒有值，錯誤原因寫入 `custom_records.formula_errors`（系統資料類別為該資料表的 `formula_errors`；`{ "<field_key>": "除數不可為零" }`），回應帶 `formula_errors`（§10）。資料庫中的公式無法解析或有循環引用（資料不一致）時，相關欄位同樣記錄錯誤。
- 操作紀錄的 diff 包含公式欄位值的變動（公式的值存在 `data`）；`formula_errors` 不記錄。
- 不做：被引用資料變動時自動重算。

**桌面端即時預覽**（Tauri command `preview_formulas`，`apps/desktop/src-tauri/src/formula.rs`；前端在 `records/formulaPreview.ts`）：

- 輸入欄位定義（主表與有效的明細子表）、本筆資料與明細的 JSON 存值，可另外提供關聯目標資料的快照；以 `hammer_formula::record` 計算，與伺服器存檔時的結果一致（`crates/formula/testdata/record_preview.json` 為兩邊共用的測試資料）。`TODAY()` 為電腦目前時間在 Asia/Taipei 的日期。
- 表單：公式欄位唯讀，輸入其他欄位或明細時更新（停止輸入 150 毫秒後）；明細的公式與主表的子表彙總一起更新。預覽的錯誤顯示錯誤提示，不阻擋存檔；存檔時以伺服器計算的結果為準。
- 關聯引用：表單不讀取關聯資料的最新值（不提供快照）。直接或間接使用關聯引用的公式欄位（含子表彙總讀取的明細公式）顯示上一次存檔的值與錯誤；表單有未存檔的修改時標示「存檔後更新」。
- 列表與單筆顯示：公式欄位依 `formula_result_type` 格式化；`formula_errors` 有該欄位時顯示錯誤圖示，滑鼠移上去顯示原因。篩選與排序依結果型態（§10）。

**重新計算**（`hammer_core::recalculate`，`POST /api/v1/entities/{key}/recalculate`，§10）：

- 需要 `schema.manage`。只能對主表（`master` / `document`）執行，明細隨主表一起重算；明細子表、不存在或已停用的資料類別回傳 `404`。系統資料類別（M3 起）也可以執行，處理其專屬資料表與系統明細子表。
- 處理該資料類別所有**未刪除**的主表資料與其未刪除的明細（只有有效的明細子表）。計算方式與「存檔時計算」相同（先明細、再主表；`TODAY()` 為執行時間在 Asia/Taipei 的日期）。
- 同步執行，依 `id` 由小到大每 500 筆主表資料一個 transaction（鎖定該批主表與明細，並以 `FOR SHARE` 鎖定資料類別）；每批重新讀取欄位定義。
- 主表或任一列明細的值（含 `formula_errors`）有變動時寫入：主表的 `version` + 1，有變動的明細也 + 1；都沒有變動的資料不寫入、`version` 不變。
- 回傳處理的主表筆數、有變動的主表筆數、重算後主表或明細有公式錯誤的主表筆數。
- 操作紀錄不逐筆寫：整批完成時在最後一批的 transaction 內寫一筆 `audit_logs`（動作 `recalculate`、`entity_key` 為資料類別代碼、`record_id` 為空、`after_data` 為 `{ "processed", "changed", "with_errors" }`）。中途失敗時已完成的批次不還原、也不寫操作紀錄；再執行一次即可（沒有變動的資料不會寫入）。
- 不做：背景作業與進度回報、只重算部分資料。

### 7.7 表單版面（M8）

管理者可以自訂每個資料類別的表單畫面（桌面程式的 `RecordFormPage`：主檔與單據的新增、編輯、檢視共用）：把欄位分成多個區塊、每個區塊 1–4 欄、調整欄位寬度與順序，並移動系統區塊（明細表、挑選出貨單等）的位置。版面存在 `form_layouts`（§8.5），每個資料類別最多一個、只有全域版面；型別在 `hammer_shared::form_layout`，檢查與正規化在 `hammer_core::form_layouts`。

**不做**：個人表單版面、分頁（Tabs）、明細表內部的欄位順序與欄寬（仍依明細子表的欄位定義 `sort_order`、`column_width`）、依條件顯示欄位。

#### config 結構

```json
{
  "sections": [
    {
      "key": "basic",
      "title": "基本資料",
      "columns": 2,
      "collapsible": false,
      "collapsed": false,
      "items": [
        { "type": "block", "block": "record_no" },
        { "type": "field", "field": "receipt_date", "span": 1 },
        { "type": "field", "field": "supplier_id", "span": 1 },
        { "type": "spacer", "span": 1 },
        { "type": "field", "field": "currency_code", "span": 1 },
        { "type": "field", "field": "remark", "span": 2 }
      ]
    },
    {
      "key": "lines",
      "title": "明細",
      "columns": 1,
      "collapsible": false,
      "collapsed": false,
      "items": [
        { "type": "block", "block": "lines:goods_receipt_line" }
      ]
    },
    {
      "key": "extra",
      "title": "其他資訊",
      "columns": 3,
      "collapsible": true,
      "collapsed": true,
      "items": [
        { "type": "field", "field": "inspector", "span": 1 },
        { "type": "field", "field": "inspection_note", "span": 2 }
      ]
    }
  ]
}
```

**區塊**（`sections`，依陣列順序由上往下顯示；至少 1 個、最多 20 個）：

| 屬性 | 規則 |
|---|---|
| `key` | 版面內不可重複；`^[a-z][a-z0-9_]{0,49}$`。例外：`_other`、`_blocks` 是讀取時自動附加的區塊（見下方），寫入時可以出現，由伺服器改名後儲存 |
| `title` | 去除前後空白後最長 50 字元，不可有控制字元；可以是空字串（不顯示標題） |
| `columns` | 1–4 的整數 |
| `collapsible` | 是否可以收折；為 `true` 時 `title` 不可為空 |
| `collapsed` | 開啟表單時是否預設收折；只有 `collapsible = true` 時可以為 `true` |
| `items` | 項目陣列，可以為空；同一個版面所有區塊的項目合計最多 300 個 |

**項目**（`items`，在區塊內由左而右、由上而下排列，放不下就換行；`span` 為佔用的欄數）：

| `type` | 其他屬性 | 規則 |
|---|---|---|
| `field` | `field`、`span` | `field` 為此資料類別**主表**的欄位代碼（不含明細子表的欄位）；整個版面中同一個欄位最多出現一次；`span` 為 1 到該區塊 `columns` 的整數，省略時為 1 |
| `spacer` | `span` | 空白佔位，用來讓下一個欄位換到下一欄或下一列；`span` 規則同上 |
| `block` | `block` | 系統區塊代碼（見下方）；一律佔滿整列（不接受 `span`）；整個版面中同一個系統區塊最多出現一次 |

- 欄位的顯示與否仍由欄位定義的 `is_visible` 決定，**版面不能隱藏欄位**。`is_visible = false` 的欄位可以放在版面中（保留位置，改回顯示時就出現在該位置），但不顯示。
- 欄位的標籤、必填、唯讀、說明文字、輸入元件都依欄位定義，版面只決定位置與寬度。
- 依附在某個欄位的說明（例如銷項發票、進項憑證 `tax_period` 下方的期別說明）跟著該欄位移動，不是系統區塊。

#### 系統區塊

系統區塊是表單中不屬於欄位定義、由程式提供的區塊。系統區塊**只能移動、不能移除**：寫入的版面可以不放，讀取時自動附加（見下方「自動附加」）。目前 `RecordFormPage` 內寫死的區塊對應如下：

| 代碼 | 名稱 | 適用的資料類別 | 何時顯示（由區塊本身判斷） | 自動附加的位置 |
|---|---|---|---|---|
| `allowance_available` | 可折讓金額 | `sales_allowance`、`purchase_allowance` | 可以編輯且已選擇發票（憑證）時 | 欄位之前 |
| `record_no` | 單號 | 所有主檔與單據（資料類別的單號規則可以隨時設定或清除，所以一律適用） | 有單號規則，或這筆資料已有單號；新增時顯示「存檔後產生」 | 欄位之前 |
| `shipment_picker` | 選擇出貨單 | `sales_invoice` | 可以編輯時 | 欄位之後 |
| `receipt_picker` | 選擇進貨單 | `purchase_invoice` | 可以編輯時 | 欄位之後 |
| `lines:<明細子表代碼>` | 明細表（標題為明細子表名稱） | 每個有效的明細子表一個，例如 `lines:goods_receipt_line`、`lines:customer_contact`、自訂的 `lines:parts`；依明細子表的 `sort_order` | 一律顯示 | 欄位之後 |
| `used_lots` | 實際扣用的批號（出貨單）／加回的批號（銷貨退回） | `shipment`、`sales_return` | 已過帳或已作廢時 | 欄位之後 |

上表的順序即自動產生時的順序（與 M8 之前的 `RecordFormPage` 相同）。

- 「只在新增時出現」「只在某些狀態出現」的區塊，由區塊本身依上表判斷；版面只決定位置，新增、編輯、檢視共用同一個版面。
- 各單據的**帶入工具**（`PurchasingAutofill`、`SalesAutofill`、`PurchaseReturnAutofill`、`SalesReturnAutofill`、`SalesInvoiceAutofill`、`PurchaseInvoiceAutofill`：選擇廠商、客戶、訂單、進貨單、出貨單、幣別等之後自動帶入其他欄位與明細）沒有畫面，只監聽欄位變動，所以不是系統區塊，不受版面影響。銷貨折讓、進貨折讓的帶入工具有畫面的部分即 `allowance_available`。
- 表單以外的頁面元素不屬於版面：頁首的標題與動作按鈕（過帳、作廢、確認、結案、刪除）、單據狀態說明、發票號碼（`InvoiceLinksNote`）、錯誤與版本衝突提示、表單最下方的「儲存」按鈕。
- 適用的資料類別、順序與位置定義在 `hammer_shared::form_layout::system_blocks`，伺服器與桌面程式共用；`lines:*` 依資料類別目前有效的明細子表決定。之後新增寫死的區塊時，同步更新本表與該清單。

#### 寫入時的檢查

`PUT` 的 `config` 必須完全合法，任一項不符 → `422 VALIDATION`，`details.field` 指出位置（例如 `sections[1].items[3].field`）：

- 結構與上表的規則（`key` 格式與重複、`title` 長度、`columns`、`span` 範圍、`collapsible` / `collapsed`、數量上限）
- `field` 必須是此資料類別主表**有效**（`is_active = true`）的欄位；不存在、已停用、重複 → `422`
- `block` 必須是此資料類別適用的系統區塊；`lines:<代碼>` 必須是此資料類別有效的明細子表；不適用、重複 → `422`
- 不認得的屬性 → `422`（`deny_unknown_fields`）

#### 讀取時的處理（正規化）

`GET` 回傳的 `config` 是「正規化」後的版面，桌面程式（表單渲染與設計器）直接使用，不需要自己處理以下規則：

1. **略過**：參照到已停用或不存在的欄位、已停用或刪除的明細子表、不再適用的系統區塊（例如欄位型態變更、明細子表停用後），讀取時略過該項目，**不回傳錯誤**。項目的 `span` 大於區塊 `columns` 時調整為 `columns`。資料庫中的 config 格式不合法（無法解析）時視為沒有設定版面。
2. **自動附加欄位**：主表有效、`is_visible = true`、但沒有放進版面的欄位，依欄位 `sort_order` 附加到最後的「其他欄位」區塊（`key = "_other"`、`title = "其他欄位"`、`columns = 1`、不可收折），每個欄位 `span = 1`。沒有要附加的項目時不產生這個區塊。
3. **自動附加系統區塊**：此資料類別適用、但沒有放進版面的系統區塊依上表的順序：「欄位之前」的插入第一個區塊的最前面；「欄位之後」的附加到最後，接在「其他欄位」之後，放在 `key = "_blocks"`、`title` 為空、`columns = 1` 的區塊。
4. 略過之後沒有任何項目的區塊仍然回傳（設計器要能看到），桌面程式渲染表單時不顯示沒有可見項目的區塊。

**沒有設定版面**（或已還原成預設）時，依欄位定義自動產生，與 M8 之前的畫面相同：一個 `key = "main"`、`title` 為空、`columns = 1`、不可收折的區塊，依序放「欄位之前」的系統區塊、主表有效且顯示的欄位（依 `sort_order`）、「欄位之後」的系統區塊。回應以 `is_default: true` 表示是自動產生的版面。

自動附加與自動產生的 `_other`、`_blocks`、`main` 區塊回到設計器後就是一般的區塊；管理者存檔時這些項目就寫入版面（寫入前伺服器把 `_other`、`_blocks` 改名為 `other`、`blocks`，已有同名的區塊時依序加上 `_2`、`_3`）。

#### 渲染

- 區塊以標題分隔，區塊內為 `columns` 欄的格線，項目依 `span` 佔用欄數。可收折的區塊依 `collapsed` 決定開啟時是否收折；收折的區塊內有驗證錯誤時，送出時自動展開。
- **視窗變窄時退回單欄**：表單內容寬度小於 `columns × 280px` 時，該區塊改為單欄（`span` 視為 1、`spacer` 不顯示），項目順序不變。
- 唯讀者（沒有 `record.write`）看到的版面與可編輯者相同。

#### 權限與紀錄

- 讀取需要 `record.read`（打開表單的人都要讀取版面）；修改、還原成預設需要 `schema.manage`。
- 修改、還原寫入 `audit_logs`（`entity_key = "form_layout"`、`record_id` 為 `form_layouts.id`；`before_data` / `after_data` 為 config）；還原成預設時 `after_data` 為 `null`。
- 樂觀鎖定（§9.4）：`PUT` 帶 `version`；第一次建立時 `version` 為 `null`（此時已有版面 → `409 CONFLICT`）。

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
`draft → confirmed → partially_done → closed / void`（採購單的 `partially_done` 為 `partially_received`，轉換見 §8.8「採購單的狀態」；訂單為 `partially_shipped`，見 §8.9「訂單的狀態」）。

### 8.3 ER 概觀

```
suppliers ─┬─< purchase_orders ─< purchase_order_lines
           │                          ┊ purchase_order_line_id（可為空）
           ├─< goods_receipts ──< goods_receipt_lines ──0..1── inventory_lots（service 沒有批號）
           │                          ┊ goods_receipt_line_id
           └─< purchase_returns ─< purchase_return_lines ─< lot_allocations
                                                                  │
customers ─┬─< sales_orders ────< sales_order_lines               │
           │                          ┊ sales_order_line_id（可為空）
           ├─< shipments ───────< shipment_lines ─< lot_allocations >─┤
           │                          ┊ shipment_line_id              │
           └─< sales_returns ──< sales_return_lines（加回原批號，記在 inventory_transactions）
                                                                  │
products ─────────────────────────────────────< inventory_transactions

customers ─< sales_invoices ─┬─< sales_invoice_lines
                             ├─< sales_invoice_shipments >── shipments
                             └─< sales_allowances ─< sales_allowance_lines（0..1 ── sales_returns）
suppliers ─< purchase_invoices ─┬─< purchase_invoice_receipts >── goods_receipts
                                └─< purchase_allowances（0..1 ── purchase_returns）

stock_adjustments ─< stock_adjustment_lines ─┬─0..1── inventory_lots（盤盈：新批號）
                                             └─< lot_allocations（盤虧）
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
| action | TEXT | `create` / `update` / `delete` / `post` / `void` / `login` / `login_failed` / `logout` / `password_change` / `export` / `recalculate` / `confirm` / `close`（常數在 `hammer_shared::audit`；`recalculate` 為重新計算公式，整批一筆、`record_id` 為空） |
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
- 第一版自訂資料類別沒有作廢狀態；系統單據的單號見 §7.1「系統單據」（存在單據資料表的 `record_no`，預設規則不可清空）。

#### `attachments`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_key / record_id / field_key | | 附件屬於哪筆資料的哪個欄位 |
| file_name / mime_type / size_bytes | | |
| sha256 | TEXT | |
| storage_path | TEXT | 相對於附件目錄的路徑，以附件 id 命名 |
| ＋共通欄位 | | |

索引：`(entity_key, record_id)`、未綁定附件的部分索引 `(created_at) WHERE record_id IS NULL`。`entity_key`、`record_id`、`field_key` 三者同時有值（已綁定）或同時為 `NULL`（未綁定）。

**規則**（`file` 欄位的用法見 §7.3）：

- 上傳時計算 SHA-256；單檔大小上限由環境變數 `HAMMER_ATTACHMENT_MAX_BYTES` 設定（預設 20 MiB），超過回傳 `413 PAYLOAD_TOO_LARGE`。上傳的路由不受一般 request 的 body 上限（`HAMMER_BODY_LIMIT_BYTES`）限制。
- 檔名只存在資料庫（去除路徑與控制字元，最多 255 字元）；磁碟上以附件 id 命名，避免路徑穿越。上傳中的檔案寫在附件目錄的 `tmp/`，完成後才移到正式位置。MIME 使用上傳時的 `Content-Type`，沒有或格式不正確時為 `application/octet-stream`。
- 欄位值被換掉或清空時標記 `deleted_at`，檔案保留；資料被軟刪除時附件不變。
- 未綁定超過 24 小時的附件由清理函式刪除資料列與檔案（伺服器啟動時與之後每 24 小時執行一次）；同時刪除超過 24 小時的暫存檔。已綁定的附件（含已標記刪除的）不清理。
- 操作紀錄（`entity_key = attachments`、`record_id` 為附件 id）：上傳 `create`、綁定 `update`、換掉或清空 `delete`；清理 `delete`，操作者為 `NULL`。

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
| parent_entity_id | UUID FK → self | 明細子表所屬的主表（`kind = detail` 時必填，其他為空）；管理者建立的明細子表，主表必須是有效的自訂 `master` 或 `document`，不可是另一個明細子表（系統明細子表由 migration 建立，§8.6）；建立後不可更換 |
| numbering_pattern | TEXT | 單號規則；明細子表、主檔類的系統資料類別不可設定；系統單據必須設定（不可清空，§7.1） |
| title_template | TEXT | 資料標題範本，例如 `{record_no} {name}`；規則見下方 |
| sort_order | INTEGER | 選單排序 |
| is_active | BOOLEAN | |
| ＋共通欄位 | | |

**資料標題**（`title_template`，解析與產生在 `hammer_core::title`）：關聯欄位、選擇器、操作紀錄用來表示「這筆資料叫什麼」的文字。通用資料 API 的列表與單筆回應都有 `title`。

- 範本中 `{}` 內可寫 `{record_no}`（單號）、本資料類別的欄位代碼 `{field_key}`，或關聯引用 `{relation_field.field_key}`（見下方）；`{}` 以外的文字原樣保留。長度上限 200 字元，不可有單獨的 `{` `}` 或控制字元。
- 建立與修改資料類別時檢查：只能引用 `record_no` 或此資料類別已定義的欄位（含已停用）；新的資料類別還沒有欄位，只能引用 `{record_no}`。不合法 → `422 VALIDATION`（`details.field = title_template`）。空白視為沒有範本。
- **關聯引用** `{relation_field.field_key}`（M3 起；例如商品的 `{code} {name}（{unit_id.name}）`、自訂單據的 `{record_no} {customer.short_name}`）：
  - 只能一層：`relation_field` 必須是本資料類別已定義（含已停用）、型態為 `relation` 的欄位；`field_key` 必須是它的目標資料類別已定義（含已停用）的欄位，或 `record_no`。檢查不合法 → `422 VALIDATION`（`details.field = title_template`）。
  - 值依目標欄位的型態套用下表的格式化；目標欄位本身是 `relation` 或 `file` 時為空字串（不會再往下一層）。
  - 讀取目標資料**目前**的值；目標已軟刪除時仍讀取其資料（與公式的關聯引用相同，§7.6）。
  - 以下情況為空字串：關聯欄位沒有值、找不到目標資料、關聯欄位或目標欄位已停用、關聯欄位已變更為其他型態（§7.4）。
  - 不使用目標資料的標題範本。產生「關聯目標的標題」（`relations.<field>.title`、選擇器）時，目標範本中的關聯引用也照常讀取；每一次產生標題最多多讀一層資料，不會遞迴。
  - 列表產生標題時，依關聯欄位分組，以 id 批次讀取目標資料（不逐筆查詢）。
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
  | `formula` | 依 `formula_result_type` 套用上列規則；計算失敗（存值為 `null`）時為空字串 |
  | `relation`、`file` | 空字串（關聯目標的欄位請用關聯引用） |

- 空值、引用已停用的欄位、存值格式不正確 → 空字串；結果去除前後空白。範本產生的結果是空字串時就回傳空字串，不改用預設規則。
- 沒有設定範本時，依序使用第一個有值的：單號 → 排序最前面、有效的 `text` 欄位的值 → id 前 8 碼。資料庫中的範本不合法（加入檢查前存入的）時也使用預設規則。
- 標題在讀取時產生，不存入資料庫；第一版不能依 `title` 排序或篩選。

**資料類別管理**（邏輯在 `hammer_core::schema`，型別與端點在 `hammer_shared::schema`；寫入都需要 `schema.manage`，每個有變動的資料類別寫入一筆 `schema_change_logs` 與 `audit_logs`）：

- `kind`：`master`（主檔）、`document`（單據）、`detail`（明細子表）。`document` 的行為與 `master` 相同（可設定單號規則、可有明細子表、可作為關聯目標），差別只在選單分組顯示。系統單據（`is_system = true` 的 `document`）另有狀態、過帳與作廢（§7.1「系統單據」）。
- 列表（`GET /api/v1/entities`）：預設只列出啟用中的資料類別（選單用，需要登入）；`?include_inactive=true` 含已停用的（管理畫面用，需要 `schema.manage`）。已刪除的一律不列出。
- 修改（`PATCH /api/v1/entities/{key}`）：名稱、圖示、單號規則、標題範本、選單排序、是否啟用。停用後不出現在選單，資料 API 回傳 `404`；改回啟用即恢復。系統資料類別不可停用。
- 選單排序：`PUT /api/v1/entities/order` 一次調整多筆 `sort_order`，只需列出要調整的資料類別；任一筆版本不符整批不寫入。`order` 也是合法的代碼，同一路徑的 GET / PATCH / DELETE 轉給該資料類別。
- 刪除（`DELETE /api/v1/entities/{key}?version=N`，軟刪除）：
  - 系統資料類別不可刪除（`422`，`details.field = is_system`）。
  - 其他有效資料類別的有效 `relation` 欄位還指向它時拒絕（`422`，`details` 為 `{ "field": "relation_entity_id", "fields": [{ "entity_key", "entity_label", "field_key", "field_label" }] }`）；自己引用自己的欄位不影響。新增關聯欄位時以 `FOR SHARE` 鎖定目標資料類別，避免檢查後被刪除。
  - 它的明細子表在同一個 transaction 內一起刪除。
  - 資料（`custom_records`）與欄位定義保留，但資料類別刪除後一律視為不存在；移除它（與明細子表）所有欄位的動態索引（§7.5）。
  - `key` 的 UNIQUE 限制包含已刪除的資料類別，刪除後代碼不可再用。第一版不提供復原與永久刪除。

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
| relation_entity_id | UUID FK → entity_definitions | relation 使用；建立後不可修改，限制見 §7.3 |
| formula | TEXT | formula 使用 |
| formula_result_type | TEXT | formula 的結果型態 |
| formula_deps | JSONB | 解析後的相依欄位，供相依圖與停用檢查使用；儲存公式時由伺服器產生；格式見下方 |
| ui_widget | TEXT | 選用的輸入元件 |
| column_width | INTEGER | 列表欄寬 |
| help_text | TEXT | |
| sort_order | INTEGER | |
| is_active | BOOLEAN | |
| ＋共通欄位 | | |

`formula_deps` 的格式（`hammer_formula::dependencies` 產生，排序並去除重複）：公式引用的欄位陣列，每個元素是下列其中一種：

```json
[
  {"kind": "field", "field": "qty"},
  {"kind": "relation", "relation": "customer", "field": "discount_rate"},
  {"kind": "detail", "detail": "lines", "field": "amount"}
]
```

| kind | 公式語法 | 相依圖中指向的欄位 |
|---|---|---|
| `field` | `{qty}` | 同一張表（主表或該明細子表）的 `qty` |
| `relation` | `{customer.discount_rate}` | 同一張表的關聯欄位 `customer`；目標欄位在其他資料，不參與排序 |
| `detail` | `SUM(lines.amount)` | 明細子表 `lines` 的 `amount`（只有主表的公式可以使用） |

儲存公式時，以資料類別（含明細子表）所有公式欄位的 `formula_deps` 建立相依圖，有循環就拒絕並回傳循環路徑；計算順序依拓撲排序，明細的公式先於主表（`hammer_formula::calculation_order`）。

#### `schema_change_logs`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| changed_at / changed_by | | |
| target_type | TEXT | `entity` / `field` |
| target_id | UUID | |
| action | TEXT | `create` / `update` / `deactivate` / `delete`（資料類別的軟刪除） |
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

#### `form_layouts`（M8）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_id | UUID FK → entity_definitions | 主表的資料類別（`kind` 為 `master` 或 `document`；明細子表不可設定） |
| config | JSONB | 區塊與項目，格式見 §7.7 |
| ＋共通欄位 | | 含 `version`（樂觀鎖定）、`deleted_at` |

- 部分唯一索引 `(entity_id) WHERE deleted_at IS NULL`：每個資料類別最多一筆有效的版面。只有全域版面，沒有 `user_id`（不做個人表單版面）。
- 讀取需要 `record.read`，修改需要 `schema.manage`；寫入時檢查 `version`（§9.4），並寫入 `audit_logs`。
- 還原成預設時軟刪除（`deleted_at`），之後再設定時建立新的一筆。資料類別刪除時版面保留，但不會再被讀取。
- 讀取時略過不合法的參照、自動附加未放入的欄位與系統區塊、沒有版面時自動產生（§7.7「讀取時的處理」）。
- 型別與端點見 `hammer_shared::form_layout`。

#### `custom_records`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| entity_id | UUID FK → entity_definitions | |
| parent_record_id | UUID FK → self | 明細子表指向主表的資料 |
| record_no | TEXT | 依單號規則產生（如果有設定），見 §8.4 `number_sequences` |
| sort_order | INTEGER | 明細的順序（存檔時 request 陣列中的位置，從 0 開始）；主表的資料固定為 0 |
| data | JSONB | 所有欄位的值，鍵為 field_key（含公式欄位的計算結果） |
| formula_errors | JSONB | 公式欄位的計算錯誤：鍵為欄位代碼、值為錯誤原因；沒有錯誤時為 `NULL`（§7.6「存檔時計算」）。CHECK 必須是 JSON 物件 |
| ＋共通欄位 | | |

索引：`(entity_id)`、`(parent_record_id, entity_id, sort_order, id) WHERE parent_record_id IS NOT NULL`、`GIN(data)`、部分唯一索引 `(entity_id, record_no) WHERE record_no IS NOT NULL`（含已刪除的資料），以及第 7.5 節所述的動態索引。

**明細子表**（`kind = detail`；邏輯在 `hammer_core::records`、`hammer_core::record_lines`，型別在 `hammer_shared::record`）：

- 明細也存在 `custom_records`，`entity_id` 為明細子表、`parent_record_id` 為主表的資料；沒有單號。欄位定義沿用欄位 API。只支援一層明細。
- 明細**只能透過主表存取**：`/api/v1/entities/{明細子表代碼}/records`（含 `lookup`）一律 `404`。
- 順序：採用 `sort_order` 欄位（不沿用 id 順序），值為存檔時 request 陣列中的位置，讀取時依 `sort_order, id` 排序；修改時調整陣列順序即可重新排序。
- 樂觀鎖定只看主表的 `version`（§9.4）。明細有任何變動（新增、修改、刪除、順序）時，主表的 `version` 也遞增；明細本身的 `version` 在每次被修改時遞增，但不用於檢查。
- 刪除主表時，明細在同一個 transaction 內一起軟刪除；附件保留（與主表相同）。
- 「不可重複」的明細欄位：同一個明細子表的所有明細（不分主表）不可重複。同一次存檔內的重複在寫入前檢查；與其他主表的明細重複由唯一索引擋下。
- 操作紀錄：明細不另外寫紀錄，變動記在主表那筆紀錄的 diff，鍵為 `lines.<明細子表代碼>`（欄位代碼不能有 `.`，不會衝突），值為該明細子表所有明細依順序的 `[{ "id", "data" }]`；只有變動的明細子表會出現。
- 已停用的明細子表：不出現在回應的 `lines`，request 帶到時視為不存在；刪除主表時其明細仍一起軟刪除。
- Payload 格式見 §10「主表與明細」。

---

### 8.6 基本資料

#### M3 的系統資料類別

M3 的 migration 寫入下列 `entity_definitions`（`is_system = true`、`storage = table`、`is_active = true`，id 為 migration 中固定的 UUID）。規則見 §7.1；幣別、匯率、稅別**不是**資料類別，使用專屬端點（見 `currencies`、`exchange_rates`、`tax_rates`）。

| key | label | kind | table_name | 主表 | title_template | sort_order |
|---|---|---|---|---|---|---|
| `product` | 商品 | master | `products` | | `{code} {name}` | 10 |
| `product_category` | 商品分類 | master | `product_categories` | | `{name}` | 20 |
| `unit` | 單位 | master | `units` | | `{name}` | 30 |
| `customer` | 客戶 | master | `customers` | | `{code} {name}` | 40 |
| `customer_contact` | 客戶聯絡人 | detail | `customer_contacts` | `customer` | `{name}` | 0 |
| `supplier` | 廠商 | master | `suppliers` | | `{code} {name}` | 50 |
| `supplier_contact` | 廠商聯絡人 | detail | `supplier_contacts` | `supplier` | `{name}` | 0 |

`icon`、`numbering_pattern` 為 `NULL`。管理者可以修改名稱、圖示、標題範本、選單排序（§8.5「資料類別管理」），不可停用、刪除，也不可設定單號規則。

#### 系統資料表的共同規則

下列資料表（`units` 到 `supplier_contacts`）都依照以下規則：

- 欄位依序為：`id UUID PRIMARY KEY`、表中列出的系統欄位、`custom_data`、`formula_errors`、共通欄位（§8.1）：
  ```sql
  custom_data    JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
  formula_errors JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
  ```
- 建立 `set_updated_at` trigger、`GIN (custom_data)` 索引、每個外鍵欄位的索引。
- 表中「定義」欄寫「必填」的欄位為 `NOT NULL`，其他可為 `NULL`；文字欄位沒有值時存 `NULL`，不存空字串。
- 「唯一」：`CREATE UNIQUE INDEX … ON <table> (lower(code)) WHERE deleted_at IS NULL`（不分大小寫、只限未刪除的資料，§7.2）。
- 每個系統欄位在 `field_definitions` 寫入一筆：`is_system = true`、`storage = column`、`field_key` 與 `column_name` 都是欄位名稱、`label` 為表中的「顯示名稱」、`sort_order` 依表中順序為 10、20、30…、`is_visible = true`、`is_active = true`；「定義」欄的寫法對應如下：

  | 寫法 | 欄位定義 |
  |---|---|
  | 必填 | `is_required = true`（不可取消，§7.2） |
  | 唯一 | `is_unique = true`，並有上述唯一索引 |
  | 可搜尋 | `is_searchable = true`，並有 `(lower(<欄位>))` 索引（`code` 由唯一索引涵蓋） |
  | 小數 n | `decimal_places = n`（與實體欄位的小數位數相同） |
  | `min 0`、`max_length 50` | `validation`，例如 `{"min":"0"}`、`{"max_length":50}` |
  | 預設 x | `default_value` |
  | → `unit` | `relation_entity_id` 為該系統資料類別 |
  | `ui_widget = currency` | 桌面程式以幣別選單輸入（`GET /api/v1/currencies`） |

- 明細資料表（聯絡人）的父資料欄位（`customer_id` / `supplier_id`）與 `sort_order` 不是欄位，不寫入 `field_definitions`；欄位定義的 `entity_id` 為明細子表（`customer_contact` / `supplier_contact`）。
- 操作紀錄的 `entity_key` 為資料類別代碼（例如 `customer`），diff 的鍵為 `field_key`（系統欄位與自訂欄位一起）；聯絡人的變動記在主表的紀錄（`lines.customer_contact`，§8.5 `custom_records`）。

#### `units`
| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| code | TEXT | `text` | 代碼 | 必填、唯一、`max_length 50`；例如 `pcs`、`box` |
| name | TEXT | `text` | 名稱 | 必填、可搜尋；例如 個、箱 |

#### `product_categories`
| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| name | TEXT | `text` | 名稱 | 必填、可搜尋 |
| parent_id | UUID FK → product_categories | `relation` | 上層分類 | → `product_category`；不可形成循環（§7.2）；支援多層分類 |
| sort_order | INTEGER | `integer` | 排序 | 必填、預設 `0`；同一層分類的顯示順序 |

#### `products`
| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| code | TEXT | `text` | 料號 | 必填、唯一、`max_length 50` |
| name | TEXT | `text` | 品名 | 必填、可搜尋 |
| spec | TEXT | `text` | 規格 | |
| product_type | TEXT CHECK | `select` | 類型 | 必填、預設 `goods`；選項 `goods`（實體商品，有批號和庫存）/ `service`（服務，無庫存） |
| category_id | UUID FK → product_categories | `relation` | 分類 | → `product_category` |
| unit_id | UUID FK → units | `relation` | 單位 | 必填、→ `unit` |
| barcode | TEXT | `text` | 條碼 | 可搜尋 |
| tax_type | TEXT CHECK | `select` | 稅別 | 必填、預設 `taxable`；選項 `taxable`（應稅）/ `zero_rated`（零稅率）/ `exempt`（免稅），與 `tax_rates.code` 相同 |
| default_sale_price | NUMERIC(18,4) | `money` | 預設售價 | 小數 4、`min 0`；台幣、**未稅**（銷貨單據的帶入見 §8.9「金額與稅額」） |
| safety_stock | NUMERIC(18,4) | `decimal` | 安全庫存 | 小數 4、`min 0` |
| is_active | BOOLEAN | `boolean` | 啟用 | 必填、預設 `true` |
| notes | TEXT | `long_text` | 備註 | |

`select` 欄位的 `options` 以表中的值與括號內的名稱寫入（例如 `[{"value":"goods","label":"實體商品"},{"value":"service","label":"服務"}]`），實體欄位另有 `CHECK` 限制相同的值。

#### `currencies`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| code | TEXT UNIQUE | ISO 4217 代碼，3 個大寫英文字母（`CHECK (code ~ '^[A-Z]{3}$')`）；建立後不可修改 |
| name | TEXT NOT NULL | 例如 新台幣 |
| symbol | TEXT | 例如 `NT$` |
| decimal_places | SMALLINT NOT NULL | 金額的小數位數，0–4（外幣金額為 `NUMERIC(18,4)`，§8.1） |
| is_base | BOOLEAN NOT NULL DEFAULT false | 本位幣，只有 TWD 為 true；部分唯一索引 `(is_base) WHERE is_base` |
| is_active | BOOLEAN NOT NULL DEFAULT true | |
| created_at / created_by / updated_at / updated_by / version | | 共通欄位中除了 `deleted_at` 以外的欄位；幣別不可刪除，只能停用 |

> 主鍵依 §5 使用 uuid；其他資料表以 `code` 參照幣別（`REFERENCES currencies(code)`），單據上存的也是代碼。

**預設資料**（migration 寫入，都是啟用中）：

| code | name | symbol | decimal_places | is_base |
|---|---|---|---|---|
| TWD | 新台幣 | NT$ | 0 | true |
| USD | 美元 | US$ | 2 | false |
| JPY | 日圓 | JP¥ | 0 | false |
| EUR | 歐元 | € | 2 | false |
| CNY | 人民幣 | CN¥ | 2 | false |
| HKD | 港幣 | HK$ | 2 | false |

**維護規則**（`hammer_core::currencies`；型別在 `hammer_shared::currency`）：

- 列表 `GET /api/v1/currencies`：需要登入；預設只列出啟用中的，`?include_inactive=true` 含已停用的。筆數很少，不分頁，依 `is_base` 優先、再依 `code` 排序。
- 新增 `POST /api/v1/currencies`、修改 `PATCH /api/v1/currencies/{code}`（帶 `version`，§9.4）：需要 `settings.manage`。
  - 新增：`code` 必須是 3 個大寫英文字母（輸入的小寫轉成大寫），不可與既有幣別重複（含已停用）；只檢查格式，不內建 ISO 4217 清單。
  - 可修改 `name`、`symbol`、`decimal_places`、`is_active`；`code`、`is_base` 不可修改。`decimal_places` 只影響之後輸入的金額，已存的金額不變。
  - 停用：TWD 不可停用（`422`，`details.field = is_active`）。停用的幣別不出現在幣別選單、不可用於新的資料（廠商的 `default_currency_code`、M4 的單據），也不可新增或修改它的匯率；已存的資料、匯率保留，可以查看。重新啟用即恢復。
- 不可刪除。
- 寫入操作紀錄（`entity_key = currencies`、`record_id` 為幣別的 `id`）。

#### `exchange_rates`
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PK | |
| currency_code | TEXT NOT NULL FK → currencies(code) | 不可為本位幣（TWD） |
| rate_date | DATE NOT NULL | 匯率適用的日期（Asia/Taipei） |
| rate | NUMERIC(18,6) NOT NULL CHECK (rate > 0) | 1 外幣 = ? 台幣 |
| ＋共通欄位 | | |

索引：部分唯一索引 `(currency_code, rate_date) WHERE deleted_at IS NULL`（刪除後同一天可以重新輸入），也用於查詢適用的匯率。

> 單據會帶入適用的匯率作為預設值，使用者可以修改。單據上存的是實際使用的匯率，之後修改匯率表不會影響已建立的單據。

**維護規則**（`hammer_core::exchange_rates`；型別在 `hammer_shared::exchange_rate`）：

- 權限（§6.1）：查看需要 `record.read`；新增、修改、刪除需要 `record.write`。
- 列表 `GET /api/v1/exchange-rates?currency_code=&date_from=&date_to=&page=&page_size=`：分頁（§10），只列出未刪除的，依 `rate_date DESC, currency_code, id` 排序；日期區間含開始、含結束。
- 新增 `POST /api/v1/exchange-rates`（`currency_code`、`rate_date`、`rate`）：幣別必須存在、啟用中、不是本位幣；`rate` 大於 0，最多 6 位小數（超過時 `422`，不自動四捨五入）；同一幣別同一天已有未刪除的匯率時 `422 VALIDATION`（`details.field = rate_date`）。日期不限制，可以輸入未來的日期。
- 修改 `PATCH /api/v1/exchange-rates/{id}`（帶 `version`）：只能修改 `rate`；要換日期或幣別時刪除後重新新增。幣別已停用時不可修改。
- 刪除 `DELETE /api/v1/exchange-rates/{id}?version=N`：軟刪除。
- 寫入操作紀錄（`entity_key = exchange_rates`、`record_id` 為匯率的 `id`）。

**適用的匯率**（`GET /api/v1/exchange-rates/effective?currency_code=USD&date=2026-11-17`，需要 `record.read`）：

- 取該幣別 `rate_date` **小於或等於**單據日期、未刪除的匯率中 `rate_date` 最大的一筆（`ORDER BY rate_date DESC LIMIT 1`）。不使用單據日期之後的匯率。
- 回應 `{ "currency_code": "USD", "date": "2026-11-17", "rate": "32.150000", "rate_date": "2026-11-15" }`；找不到時 `rate`、`rate_date` 為 `null`（不是錯誤，單據的匯率欄位留空讓使用者輸入）。
- `currency_code` 為本位幣時固定回傳 `rate = "1.000000"`、`rate_date = null`。幣別不存在 → `422 VALIDATION`（`details.field = currency_code`）；幣別已停用仍可查詢（查看既有單據時使用）。
- 單據日期是單據上的日期欄位（Asia/Taipei，D13），不是存檔時間。

#### `tax_rates`
| 欄位 | 型態 | 說明 |
|---|---|---|
| code | TEXT PK | `taxable` / `zero_rated` / `exempt` |
| name | TEXT NOT NULL | 應稅 / 零稅率 / 免稅 |
| rate | NUMERIC(6,4) NOT NULL | 0.0500 / 0 / 0 |

由 migration 寫入，M3 不提供修改（所以不需要 uuid 主鍵與 `version`）；`GET /api/v1/tax-rates` 需要登入，回傳全部（不分頁）。商品的 `tax_type` 選項與此相同。

---

### 8.7 客戶與廠商

依照 §8.6「系統資料表的共同規則」。

#### `customers`
| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| code | TEXT | `text` | 客戶編號 | 必填、唯一、`max_length 50` |
| name | TEXT | `text` | 公司名稱 | 必填、可搜尋 |
| short_name | TEXT | `text` | 簡稱 | 可搜尋 |
| tax_id | TEXT | `text` | 統一編號 | 可搜尋；有填時必須是 8 位數字（§7.2，不驗證檢查碼）；可以不填（國外客戶沒有統一編號） |
| phone | TEXT | `text` | 電話 | |
| fax | TEXT | `text` | 傳真 | |
| email | TEXT | `text` | Email | |
| address | TEXT | `text` | 聯絡地址 | |
| invoice_title | TEXT | `text` | 發票抬頭 | |
| invoice_address | TEXT | `text` | 發票地址 | |
| payment_terms_days | INTEGER | `integer` | 付款天數 | `min 0`；月結天數 |
| credit_limit | NUMERIC(18,2) | `money` | 信用額度 | 小數 2、`min 0`；台幣 |
| is_active | BOOLEAN | `boolean` | 啟用 | 必填、預設 `true` |
| notes | TEXT | `long_text` | 備註 | |

#### `customer_contacts`（明細子表 `customer_contact`）
| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| customer_id | UUID NOT NULL FK → customers | （不是欄位） | | 所屬客戶 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序（§8.5 `custom_records`「明細子表」） |
| name | TEXT | `text` | 姓名 | 必填 |
| title | TEXT | `text` | 職稱 | |
| phone | TEXT | `text` | 電話 | |
| mobile | TEXT | `text` | 手機 | |
| email | TEXT | `text` | Email | |
| is_primary | BOOLEAN | `boolean` | 主要聯絡人 | 必填、預設 `false`；每個客戶最多一位（§7.2） |
| notes | TEXT | `long_text` | 備註 | |

索引：`(customer_id, sort_order, id) WHERE deleted_at IS NULL`。

#### `suppliers`
欄位與 `customers` 相同（`code` 的顯示名稱為「廠商編號」），在 `credit_limit` 之後另外加上：

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| default_currency_code | TEXT FK → currencies(code) | `text` | 預設幣別 | 必填、預設 `"TWD"`、`ui_widget = currency`；必須是有效的幣別（§7.2） |
| country | TEXT | `text` | 國家 | |
| bank_info | TEXT | `long_text` | 匯款資訊 | |

#### `supplier_contacts`（明細子表 `supplier_contact`）
欄位與 `customer_contacts` 相同（`customer_id` 改為 `supplier_id`，FK → suppliers）。

---

### 8.8 進貨模組

> M4b 定案。單號、明細、狀態限制、過帳與作廢的共通規則以 §7.1「系統單據」、§10「單據動作」為準；庫存（批號、批號扣除、異動、鎖定順序）以 §8.10 為準。邏輯在 `hammer_core::purchasing`（金額計算、採購單、進貨單、進貨退回），型別在 `hammer_shared::document`。

#### 資料類別

M4b 的 migration 寫入下列 `entity_definitions`（`is_system = true`、`storage = table`、`is_active = true`、`icon` 為 `NULL`，id 為 migration 中固定的 UUID）：

| key | label | kind | table_name | 主表 | numbering_pattern | title_template | sort_order |
|---|---|---|---|---|---|---|---|
| `purchase_order` | 採購單 | document | `purchase_orders` | | `PO{yyyyMM}-{0000}` | `{record_no} {supplier_id.name}` | 110 |
| `purchase_order_line` | 採購明細 | detail | `purchase_order_lines` | `purchase_order` | | `{product_id.code} {product_id.name}` | 0 |
| `goods_receipt` | 進貨單 | document | `goods_receipts` | | `GR{yyyyMM}-{0000}` | `{record_no} {supplier_id.name}` | 120 |
| `goods_receipt_line` | 進貨明細 | detail | `goods_receipt_lines` | `goods_receipt` | | `{product_id.code} {product_id.name} {lot_no}` | 0 |
| `purchase_return` | 進貨退回 | document | `purchase_returns` | | `PR{yyyyMM}-{0000}` | `{record_no} {supplier_id.name}` | 130 |
| `purchase_return_line` | 進貨退回明細 | detail | `purchase_return_lines` | `purchase_return` | | `{product_id.code} {product_id.name} {lot_no}` | 0 |

六張表都依照 §8.6「系統資料表的共同規則」（欄位順序、`custom_data`、`formula_errors`、共通欄位、`set_updated_at` trigger、`GIN (custom_data)`、外鍵索引、`field_definitions` 的寫法），以及 §7.1「系統單據」（`record_no` 與唯一索引 `uq_<資料表>_record_no`、`sort_order`、父資料欄位）。主表另外都有：

- `posted_at` / `posted_by`、`voided_at` / `voided_by`（不是欄位），CHECK 與盤點調整單相同：`CHECK ((status = 'draft') = (posted_at IS NULL))`、`CHECK ((status = 'void') = (voided_at IS NOT NULL))`、`CHECK ((posted_at IS NULL) = (posted_by IS NULL))`、`CHECK ((voided_at IS NULL) = (voided_by IS NULL))`。採購單的 `posted_at` / `posted_by` 記錄**確認**的時間與人員。採購單、進貨單另有 `CHECK (status = 'draft' OR exchange_rate IS NOT NULL)`（確認、過帳後必須有匯率）。
- 索引：`(<單據日期>, id)`、`(status)`、`(supplier_id)`；明細 `(<主表單數>_id, sort_order, id) WHERE deleted_at IS NULL`、`(product_id)`。

表中「系統維護」的欄位依 §7.1：`is_readonly = true`，request 帶了一律忽略，由伺服器在存檔、確認、過帳時寫入。

#### `purchase_orders`（採購單，系統單據 `purchase_order`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| supplier_id | UUID NOT NULL FK → suppliers | `relation` | 廠商 | 必填、→ `supplier` |
| order_date | DATE NOT NULL | `date` | 採購日期 | 必填；匯率的適用日期。桌面程式新增時預設今天 |
| expected_date | DATE | `date` | 預計到貨日 | |
| currency_code | TEXT NOT NULL FK → currencies(code) | `text` | 幣別 | 必填、`ui_widget = currency`；見「幣別與匯率」 |
| exchange_rate | NUMERIC(18,6) CHECK (exchange_rate > 0) | `decimal` | 匯率 | 小數 6、`min 0`；草稿可以留空；見「幣別與匯率」 |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 必填；選項與商品的 `tax_type` 相同；見「金額與稅額」 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft`（草稿）/ `confirmed`（已確認）/ `partially_received`（部分進貨）/ `closed`（已結案）/ `void`（已作廢） |
| subtotal | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 未稅金額 | 系統維護、小數 4；單據幣別 |
| tax_amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 稅額 | 系統維護、小數 4；單據幣別 |
| total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 總計 | 系統維護、小數 4；單據幣別 |
| total_twd | NUMERIC(18,2) | `money` | 台幣總計 | 系統維護、小數 2；參考用，匯率留空時為 `NULL` |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | 確認、作廢的時間與人員 |
| closed_at / closed_by | TIMESTAMPTZ / UUID FK → users | （不是欄位） | | **手動**結案的時間與人員；`CHECK ((closed_at IS NULL) = (closed_by IS NULL))`、`CHECK (closed_at IS NULL OR status = 'closed')` |

#### `purchase_order_lines`（明細子表 `purchase_order_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| purchase_order_id | UUID NOT NULL FK → purchase_orders | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| product_id | UUID NOT NULL FK → products | `relation` | 商品 | 必填、→ `product`；`goods`、`service` 都可以 |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 必填、小數 4、`min 0`；單據幣別、**未稅** |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4；見「金額與稅額」 |
| received_qty | NUMERIC(18,4) NOT NULL DEFAULT 0 CHECK (received_qty >= 0) | `decimal` | 已進貨數量 | 系統維護、小數 4；進貨單過帳時增加、作廢時減少 |
| notes | TEXT | `long_text` | 備註 | |

#### `goods_receipts`（進貨單，系統單據 `goods_receipt`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| supplier_id | UUID NOT NULL FK → suppliers | `relation` | 廠商 | 必填、→ `supplier` |
| purchase_order_id | UUID FK → purchase_orders | `relation` | 採購單 | → `purchase_order`；可以不經採購單直接進貨；見「採購單轉進貨單」 |
| receipt_date | DATE NOT NULL | `date` | 進貨日期 | 必填；匯率的適用日期、批號的 `received_date`、異動的 `txn_date`。桌面程式新增時預設今天 |
| currency_code | TEXT NOT NULL FK → currencies(code) | `text` | 幣別 | 必填、`ui_widget = currency` |
| exchange_rate | NUMERIC(18,6) CHECK (exchange_rate > 0) | `decimal` | 匯率 | 小數 6、`min 0`；草稿可以留空，過帳時必填 |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 必填 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft`（草稿）/ `posted`（已過帳）/ `void`（已作廢） |
| subtotal / tax_amount / total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 未稅金額 / 稅額 / 總計 | 系統維護、小數 4；單據幣別 |
| total_twd | NUMERIC(18,2) | `money` | 台幣總計 | 系統維護、小數 2 |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(purchase_order_id)`。

#### `goods_receipt_lines`（明細子表 `goods_receipt_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| goods_receipt_id | UUID NOT NULL FK → goods_receipts | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| purchase_order_line_id | UUID FK → purchase_order_lines | `relation` | 採購明細 | → `purchase_order_line`（系統明細子表，§7.3 的例外）、`is_visible = false`；必須屬於表頭的採購單（`hammer_core` 檢查） |
| product_id | UUID NOT NULL FK → products | `relation` | 商品 | 必填、→ `product`；`goods`、`service` 都可以 |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 必填、小數 4、`min 0`；單據幣別、未稅 |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4 |
| lot_no | TEXT | `text` | 批號 | `max_length 50`；`goods` 可留空（過帳時自動產生），`service` 不可填 |
| unit_cost_twd | NUMERIC(18,4) CHECK (unit_cost_twd >= 0) | `money` | 台幣單位成本 | 系統維護、小數 4；過帳時寫入（見「進貨過帳」），草稿為 `NULL` |
| notes | TEXT | `long_text` | 備註 | |
| lot_id | UUID FK → inventory_lots | （不是欄位） | | 過帳時寫入新建的批號；`service` 為 `NULL` |

另外有索引 `(purchase_order_line_id)`、`(lot_id)`。

#### `purchase_returns`（進貨退回，系統單據 `purchase_return`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| goods_receipt_id | UUID NOT NULL FK → goods_receipts | `relation` | 進貨單 | 必填、→ `goods_receipt`；必須是已過帳的進貨單；新增後不可修改（`422`，`details.fields` 指出 `goods_receipt_id`，訊息「建立後不可更換進貨單」） |
| supplier_id | UUID NOT NULL FK → suppliers | `relation` | 廠商 | 系統維護；= 進貨單的廠商 |
| return_date | DATE NOT NULL | `date` | 退回日期 | 必填；異動的 `txn_date`。桌面程式新增時預設今天；不可早於進貨單的 `receipt_date`（`422`，「退回日期不可早於進貨日期」） |
| currency_code | TEXT NOT NULL FK → currencies(code) | `text` | 幣別 | 系統維護；= 進貨單的幣別 |
| exchange_rate | NUMERIC(18,6) NOT NULL CHECK (exchange_rate > 0) | `decimal` | 匯率 | 系統維護、小數 6；= 進貨單的匯率（退回以原進貨的匯率計價，台幣金額與批號成本一致） |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 系統維護；= 進貨單的稅別 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft` / `posted` / `void` |
| subtotal / tax_amount / total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 未稅金額 / 稅額 / 總計 | 系統維護、小數 4；單據幣別 |
| total_twd | NUMERIC(18,2) NOT NULL DEFAULT 0 | `money` | 台幣總計 | 系統維護、小數 2 |
| reason | TEXT | `text` | 退回原因 | |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(goods_receipt_id)`。

#### `purchase_return_lines`（明細子表 `purchase_return_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| purchase_return_id | UUID NOT NULL FK → purchase_returns | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| goods_receipt_line_id | UUID NOT NULL FK → goods_receipt_lines | `relation` | 進貨明細 | 必填、→ `goods_receipt_line`（系統明細子表，§7.3 的例外）、`is_visible = false`；必須屬於表頭的進貨單 |
| product_id | UUID NOT NULL FK → products | `relation` | 商品 | 系統維護、→ `product`；= 進貨明細的商品 |
| lot_no | TEXT | `text` | 批號 | 系統維護；= 進貨明細的批號（`service` 為 `NULL`） |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0，上限見「進貨退回」 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 系統維護、小數 4；= 進貨明細的單價 |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4 |
| unit_cost_twd | NUMERIC(18,4) CHECK (unit_cost_twd >= 0) | `money` | 台幣單位成本 | 系統維護、小數 4；過帳時寫入批號的成本（`service` 為進貨明細的 `unit_cost_twd`） |
| notes | TEXT | `long_text` | 備註 | |
| lot_id | UUID FK → inventory_lots | （不是欄位） | | 過帳時寫入進貨明細的 `lot_id`（`service` 為 `NULL`；不是欄位，存檔時不寫入） |

另外有索引 `(goods_receipt_line_id)`、`(lot_id)`。

#### 幣別與匯率

適用於採購單與進貨單（進貨退回一律沿用進貨單的幣別、匯率，不另外帶入）。匯率的意義與精度依 §8.1、§8.6 `exchange_rates`；單據日期為採購單的 `order_date`、進貨單的 `receipt_date`。

| 時機 | 規則 |
|---|---|
| 新增時沒有帶 `currency_code` | 帶入廠商的 `default_currency_code`（進貨單有 `purchase_order_id` 時帶入採購單的幣別） |
| 新增時沒有帶 `exchange_rate` | 帶入該幣別在單據日期的**適用匯率**（與 `GET /api/v1/exchange-rates/effective` 相同的查詢）；找不到時留空 |
| 修改時 `currency_code` 有變動、但沒有帶 `exchange_rate` | 重新帶入適用匯率（找不到時清空） |
| 修改時只有單據日期變動 | 不自動更新匯率（使用者可能已手動修改）；桌面程式在日期變動時查詢適用匯率並詢問是否套用 |
| `currency_code` 為本位幣（TWD） | `exchange_rate` 一律存 `1.000000`，request 帶了其他值也覆蓋，不回傳錯誤 |
| 使用者輸入 `exchange_rate` | 必須大於 0、最多 6 位小數（超過時 `422`，不自動四捨五入，與匯率表相同） |

- `currency_code` 必須是存在且啟用中的幣別（`422`，`details.fields` 指出 `currency_code`，訊息「幣別不存在或已停用」）。與廠商的 `default_currency_code` 相同，修改時只在值有變動時檢查，所以幣別停用後既有草稿可以保留原值。
- 匯率留空時仍可存檔草稿（`total_twd` 為 `NULL`）；**確認採購單、進貨過帳時必須有匯率**，留空 → `422 VALIDATION`，`details.fields` 指出 `exchange_rate`，訊息「找不到匯率，請輸入匯率」。
- 草稿可以修改幣別與匯率；確認、過帳後依 §7.1 的狀態限制不可修改，所以匯率在確認（採購單）、過帳（進貨單）時鎖定。之後修改匯率表不影響既有單據。
- 採購單與進貨單的匯率各自獨立：進貨單依**進貨日期**帶入當時的匯率（D11），不沿用採購單的匯率。

#### 金額與稅額

三種單據使用同一套計算（`hammer_core::purchasing::amounts`，`rust_decimal`，四捨五入一律 `MidpointAwayFromZero`）。每次存檔（新增、修改）時依明細重新計算並寫入系統維護的欄位；確認、過帳不重新計算。

- **單價一律未稅**，第一版不提供「單價含稅」的輸入方式。
- **稅別以整張單據為單位**（表頭的 `tax_type`），稅額以整張計算，不逐行計算後加總（與統一發票一張一個稅額相同）。同一次進貨有不同稅別時分開開單。
- 小數位數 `d` = 單據幣別在存檔當下的 `currencies.decimal_places`（TWD、JPY 為 0，USD 為 2）。之後修改幣別的小數位數不影響已存的金額。
- 稅率取 `tax_rates.rate`（`taxable` 0.05、`zero_rated` 與 `exempt` 0）。

| 欄位 | 計算 |
|---|---|
| 明細 `amount` | `round(qty × unit_price, d)` |
| `subtotal` | `Σ amount` |
| `tax_amount` | `round(subtotal × 稅率, d)` |
| `total` | `subtotal + tax_amount` |
| `total_twd` | `round(total × exchange_rate, 2)`；匯率留空時為 `NULL` |

- **稅別的預設值**：新增時沒有帶 `tax_type` 時，幣別為 TWD → `taxable`；其他幣別 → `zero_rated`（進口時營業稅在報關時另外繳納，不在廠商的單據上）。管理者在欄位定義設定了 `default_value` 時，以 `default_value` 為準。進貨單有 `purchase_order_id` 時帶入採購單的稅別；進貨退回一律等於進貨單的稅別。
- **台幣單位成本**（進貨單過帳時寫入明細）：`unit_cost_twd = round(unit_price × exchange_rate, 4)`。單價是未稅價，稅額不計入成本（進項稅額可以扣抵）；第一版不分攤進口費用（§13 Q3）。成本以每列的單價計算，所以 `Σ qty × unit_cost_twd` 與 `total_twd − tax_amount × exchange_rate` 可能有尾差，以批號成本為準。

**範例一：台幣、應稅**（`d = 0`、稅率 0.05、匯率 1）

| 明細 | qty | unit_price | amount |
|---|---|---|---|
| 1 | 3 | 33.3333 | round(99.9999, 0) = 100 |
| 2 | 2 | 150.5000 | round(301.0000, 0) = 301 |

`subtotal` = 401；`tax_amount` = round(20.05, 0) = 20；`total` = 421；`total_twd` = 421.00。過帳後明細 1 的 `unit_cost_twd` = 33.3333。

**範例二：美元、零稅率**（`d = 2`、匯率 32.150000）

| 明細 | qty | unit_price | amount | 過帳後 unit_cost_twd |
|---|---|---|---|---|
| 1 | 10 | 12.3450 | round(123.4500, 2) = 123.45 | round(396.89175, 4) = 396.8918 |
| 2 | 3 | 7.5000 | round(22.5000, 2) = 22.50 | round(241.125, 4) = 241.1250 |

`subtotal` = 145.95；`tax_amount` = 0；`total` = 145.95；`total_twd` = round(4692.2925, 2) = 4692.29。

#### 存檔時的檢查

違反時 `422 VALIDATION`，明細錯誤依 §10「主表與明細」的格式。關聯欄位（`supplier_id`、`product_id`、`purchase_order_id`、`goods_receipt_id`）的存在檢查依 §7.3（指向未刪除的資料，以 `FOR SHARE` 鎖定）。修改時沒有帶到明細，只在會影響明細檢查的表頭欄位有變動時以既有明細檢查；確認、過帳時一律重新檢查全部。

**採購單**

| 檢查 | 錯誤位置與訊息 |
|---|---|
| 幣別、匯率（「幣別與匯率」） | `currency_code`、`exchange_rate` |
| `qty > 0`、`unit_price ≥ 0` | `qty`「數量必須大於 0」；`unit_price`「單價不可小於 0」 |

**進貨單**

| 檢查 | 錯誤位置與訊息 |
|---|---|
| 幣別、匯率 | 同採購單 |
| `qty > 0`、`unit_price ≥ 0` | 同採購單 |
| `goods`：`lot_no` 有值時不可與該商品未刪除的批號重複；同一張單據中同一商品的同一批號（不分大小寫）只能出現一次（留空的不比對） | `lot_no`「批號已存在」「批號重複」 |
| `service`：`lot_no` 不可填 | `lot_no`「服務類商品沒有批號」 |
| 有 `purchase_order_id` 時：見「採購單轉進貨單」 | |
| 沒有 `purchase_order_id` 時：明細不可有 `purchase_order_line_id` | `purchase_order_line_id`「沒有指定採購單」 |

**進貨退回**：見「進貨退回」。

- 存檔時**不檢查**庫存（進貨退回在過帳時檢查）。
- 批號的格式、自動產生、重複的處理與盤點調整單的 `gain_new` 相同（§8.10「批號」）。

#### 採購單的狀態

```
draft ──確認──▶ confirmed ──進貨過帳──▶ partially_received ──全部進貨──▶ closed
  │                │  ▲                      │  ▲                         ▲
  │                │  └────進貨作廢───────────┘  └──────進貨作廢───────────┘（自動結案時）
  │                ├──手動結案──▶ closed ◀──手動結案──┘
  │                └──作廢──▶ void
  └── 可修改、刪除
```

| 動作 | 端點（§10「單據動作」） | 前一個狀態 → 之後 | 條件 | 寫入 |
|---|---|---|---|---|
| 確認 | `POST /api/v1/entities/purchase_order/records/{id}/confirm` | `draft` → `confirmed` | 重新執行存檔時的檢查（幣別是否啟用與存檔時相同，只在值有變動時檢查，確認時不重新檢查）；至少一列明細（「至少要有一筆明細」）；`exchange_rate` 有值；廠商、明細的商品未刪除（「廠商已刪除」「商品已刪除」） | `posted_at`、`posted_by`；操作紀錄 `confirm` |
| 手動結案 | `POST …/{id}/close` | `confirmed`、`partially_received` → `closed` | 無（不再進貨的部分視為取消） | `closed_at`、`closed_by`；操作紀錄 `close` |
| 作廢 | `POST …/{id}/void` | `confirmed` → `void` | 只有 `confirmed`（沒有任何有效的進貨）可以作廢；有進貨時請手動結案 | `voided_at`、`voided_by`；操作紀錄 `void` |
| 進貨過帳、進貨作廢 | （進貨單的動作） | 依下方「狀態的重新計算」 | | `received_qty` |

- 三個動作都需要 `record.post`，request 為 `{ "version": N }`，檢查順序、回應、錯誤格式與 §10「單據動作」相同；狀態不符 → `422 INVALID_STATUS`。採購單不支援 `post`（`404`）。
- 草稿以外的狀態都不可修改、刪除（§7.1）。第一版沒有「取消確認」與「取消結案」：確認後要修改請作廢後另開新單；誤結案時另開新單採購剩餘數量。
- **狀態的重新計算**（進貨單過帳、作廢時，在同一個 transaction 內）：
  - `closed_at` 有值（手動結案）→ 維持 `closed`。
  - 否則依所有明細：每一列都 `received_qty ≥ qty` → `closed`（自動結案，`closed_at` 維持 `NULL`）；有任何一列 `received_qty > 0` → `partially_received`；都是 0 → `confirmed`。
  - 狀態或 `received_qty` 有變動時，採購單與該明細的 `version` + 1，並在採購單寫入操作紀錄（`action = update`，diff 為 `status` 與 `lines.purchase_order_line` 的 `received_qty`）。
- 已作廢的採購單維持 `void`：作廢的條件保證它沒有任何有效的進貨。
- 操作紀錄的 `action` 新增 `confirm`、`close`（`hammer_shared::audit`）。

#### 採購單轉進貨單

進貨單以 `purchase_order_id` 與明細的 `purchase_order_line_id` 指向採購單。帶入是**桌面程式**的功能，不另外提供伺服器端點：使用者選擇採購單後，桌面程式以單筆查詢讀取採購單，產生進貨單的 request。

| 進貨單欄位 | 帶入的值 |
|---|---|
| `supplier_id`、`currency_code`、`tax_type` | 採購單的值（伺服器在新增時沒有帶到也會帶入） |
| `exchange_rate` | 不帶採購單的匯率；依進貨日期帶入適用匯率（「幣別與匯率」） |
| 明細 | 採購單中**未進貨數量** `qty − received_qty > 0` 的每一列：`purchase_order_line_id`、`product_id`、`unit_price`、`qty` = 未進貨數量；`lot_no` 留空 |

使用者可以修改數量、單價、批號，刪除不進貨的列，也可以加上沒有 `purchase_order_line_id` 的明細（例如採購單沒有的贈品、運費）。

**檢查**（存檔時與過帳時都檢查；過帳時在鎖定採購單後以最新的 `received_qty` 檢查）：

| 檢查 | 錯誤位置與訊息 |
|---|---|
| 採購單的狀態是 `confirmed` 或 `partially_received` | `purchase_order_id`「採購單尚未確認」「採購單已結案」「採購單已作廢」 |
| 進貨單的 `supplier_id`、`currency_code` 與採購單相同 | `supplier_id`「廠商與採購單不同」；`currency_code`「幣別與採購單不同」 |
| `purchase_order_line_id` 是該採購單未刪除的明細 | `purchase_order_line_id`「不是此採購單的明細」 |
| 明細的 `product_id` 與採購明細相同 | `product_id`「商品與採購明細不同」 |
| **不允許超收**：同一張進貨單中指向同一列採購明細的 `qty` 合計 ≤ `qty − received_qty` | 第一列的 `qty`「超過未進貨數量 3.0000」 |

- `received_qty` 只計算**已過帳**的進貨單；其他草稿不保留數量，所以兩張草稿可以各自輸入全部的未進貨數量，先過帳的成功，後過帳的在過帳時失敗。
- 實際收到比採購數量多時，多出的部分以沒有 `purchase_order_line_id` 的明細輸入。
- 稅別、單價可以與採購單不同（廠商實際開立的金額為準），不檢查。
- 修改採購單（草稿）、作廢、結案不影響已過帳的進貨單。

#### 進貨過帳與作廢

依 §9.1、§9.3、§10「單據動作」（`post`、`void`），鎖定依 §8.10「鎖定順序」：

- 鎖定：進貨單（`FOR UPDATE`）→ 採購單（有 `purchase_order_id` 時，`FOR UPDATE`）→ 商品（`FOR SHARE`）→ 明細的 `lot_id`（只有作廢，`FOR UPDATE`）→ 採購單明細（依 id `FOR UPDATE`）。

**過帳**：

1. 檢查 `version`、`status = draft`；重新執行存檔時的檢查（含「採購單轉進貨單」）、至少一列明細、`exchange_rate` 有值、廠商與商品未刪除。
2. 依 `sort_order` 處理每一列：
   - `unit_cost_twd = round(unit_price × exchange_rate, 4)` 寫入明細（`service` 也寫入，供參考）。
   - `goods`：`lot_no` 留空時自動產生（`{單號}-{明細序號}`，§8.10）；新增 `inventory_lots`（`source_type = goods_receipt_line`、`qty_in = qty_remaining = qty`、`unit_cost_twd`、`received_date = receipt_date`）；新增異動（`receipt`、`+qty`、`txn_date = receipt_date`、`ref_doc_type = goods_receipt`）；寫入明細的 `lot_id`。
   - `service`：不建立批號與異動。
   - 有 `purchase_order_line_id`：採購明細的 `received_qty += qty`。
   - 明細有變動時明細的 `version` + 1。
3. 重新計算採購單的狀態（「採購單的狀態」）。
4. `status = posted`、`posted_at`、`posted_by`，`version` + 1；寫入操作紀錄（`post`）。

**作廢**：

1. 檢查 `version`、`status = posted`。
2. 沒有**有效的進貨退回**（指向此進貨單、`status = posted` 的 `purchase_returns`）→ 否則 `422 VALIDATION`，`details` 為 `{ "fields": [], "documents": [ { "entity_key", "record_id", "record_no" } ] }`，訊息「此進貨單已有進貨退回 PR202611-0002，請先作廢退回單」。
   接著檢查沒有被有效的進項憑證關聯（§8.11「其他單據的作廢限制」）→ 否則 `422 VALIDATION`，訊息「此進貨單已有進項憑證 AB12345678，請先作廢憑證」。
3. `goods` 的每一列：與盤點調整單的 `gain_new` 相同（§8.10「作廢」）——批號沒有其他單據的有效異動時，新增 `void_reversal` 異動（`−qty`）、批號軟刪除；被使用時記下，全部檢查完回傳 `422 LOT_IN_USE`。
4. 有 `purchase_order_line_id` 的每一列：`received_qty −= qty`；重新計算採購單的狀態（手動結案的維持 `closed`）。
5. `status = void`、`voided_at`、`voided_by`，`version` + 1；寫入操作紀錄（`void`）。

#### 進貨退回

退回**已過帳**進貨單的部分或全部明細。建立方式與「採購單轉進貨單」相同由桌面程式帶入：選擇進貨單後，列出每一列進貨明細的**可退數量**，使用者選擇要退的列與數量。

- 只能退已過帳的進貨單：存檔與過帳時檢查進貨單 `status = posted`（`goods_receipt_id`「進貨單尚未過帳」「進貨單已作廢」）。
- 表頭的 `supplier_id`、`currency_code`、`exchange_rate`、`tax_type`，明細的 `product_id`、`lot_no`、`unit_price` 由伺服器在存檔時從進貨單、進貨明細帶入（系統維護；`lot_id` 在過帳時寫入）。金額依「金額與稅額」計算，`total_twd` 使用進貨單的匯率。
- 明細的 `goods_receipt_line_id` 必須是表頭進貨單未刪除的明細（「不是此進貨單的明細」）。同一張退回單中，同一列進貨明細只能出現一次（第二列的 `goods_receipt_line_id`「進貨明細重複」）。

**退回數量上限**（存檔時與過帳時都檢查；過帳時在鎖定進貨單後檢查）：

```
可退數量 = 進貨明細的 qty − 該進貨明細在其他「已過帳」進貨退回中的 qty 合計
```

- 超過 → `422 VALIDATION`，`qty`「超過可退數量 4.0000」。與採購單轉進貨單相同，草稿不保留數量。
- `goods` 另外受批號的 `qty_remaining` 限制（批號可能已出貨或盤虧）：過帳時不足 → `422 STOCK_INSUFFICIENT`（§10），列出所有短缺。存檔時不檢查庫存。

**過帳**（鎖定順序：退回單 → 進貨單（`FOR UPDATE`，同一張進貨單的退回依序過帳）→ 商品（`FOR SHARE`）→ 明細的批號（`FOR UPDATE`））：

1. 檢查 `version`、`status = draft`；重新執行存檔時的檢查、至少一列明細、退回數量上限。
2. 依 `sort_order` 處理每一列：
   - `goods`：`qty ≤ qty_remaining`，不足時記下短缺並繼續；足夠時新增 `lot_allocations`（`source_type = purchase_return_line`、批號的成本）、`qty_remaining −= qty`、新增異動（`purchase_return`、`−qty`、`txn_date = return_date`、`ref_doc_type = purchase_return`）；明細的 `unit_cost_twd` 寫入批號的成本。
   - `service`：不處理庫存；`unit_cost_twd` 寫入進貨明細的 `unit_cost_twd`。
3. 有短缺 → rollback，`422 STOCK_INSUFFICIENT`。
4. `status = posted`、`posted_at`、`posted_by`，`version` + 1；寫入操作紀錄（`post`）。

- 退回的**成本** = Σ 批號扣除的 `qty × unit_cost_twd`（等於原進貨的成本）；退回的**金額**是單據幣別的 `total`，台幣金額 `total_twd`。兩者都只是紀錄，應付帳款的沖銷在 M6。
- 進貨退回**不影響**採購單的 `received_qty` 與狀態：需要廠商補貨時另開採購單或進貨單。

**作廢**：已過帳的退回單沒有被有效的進貨折讓關聯時可以作廢（§9.3、§8.11「其他單據的作廢限制」：「此進貨退回已有折讓單 PA202611-0001，請先作廢折讓單」）。依各列的 `lot_allocations` 把數量加回原批號，新增 `void_reversal` 異動（`+qty`、扣除時的成本、`txn_date` 為作廢當天）。原批號不會已被刪除：進貨單有有效的退回時不可作廢（上方「進貨過帳與作廢」）。

#### 廠商、商品被進貨單據引用時的限制

`hammer_core::master_data`，補充 §7.2 的「刪除限制」「商品被庫存引用時的限制」。

| 資料 | 操作 | 規則 |
|---|---|---|
| 廠商 | 刪除 | 有任何未刪除的採購單、進貨單、進貨退回（**不論狀態**，含草稿與已作廢）指向它時不可刪除：`422 VALIDATION`，`details` 為 `{ "fields": [], "document_count": 3 }`，訊息「此廠商已有 3 張採購或進貨單據，無法刪除，請改為停用」 |
| 商品 | 刪除、`product_type` 由 `goods` 改為 `service`、修改 `unit_id` | 除了 §7.2 的庫存異動，另外在有**未結案的採購單**（`confirmed`、`partially_received`，未刪除）的明細引用時不可：`422 VALIDATION`，刪除的 `details` 為 `{ "fields": [], "open_purchase_order_count": 2 }`，訊息「此商品有 2 張未結案的採購單，無法刪除」；修改時 `details.fields` 指出該欄位 |

- 鎖定：刪除、修改時先以 `FOR UPDATE` 鎖定廠商、商品再計數；單據存檔時以 `FOR SHARE` 鎖定所指的廠商、商品（關聯檢查），確認採購單時以 `FOR SHARE` 鎖定明細的商品，所以計數後不會再有新的引用。
- 草稿、已結案、已作廢的採購單引用的商品不限制（與 §7.2 的草稿相同，確認時重新檢查「商品已刪除」）；已結案、已作廢的採購單之後顯示被刪除的商品時，關聯欄位的 `deleted = true`（§7.3）。
- 停用（`is_active = false`）不受限制；停用的廠商、商品仍可以在單據上選用既有的值（§7.1：寫入關聯欄位時不檢查 `is_active`）。

---

### 8.9 銷貨模組

> M4c 定案。單號、明細、狀態限制、過帳與作廢的共通規則以 §7.1「系統單據」、§10「單據動作」為準；庫存（批號、批號扣除、異動、鎖定順序）以 §8.10 為準；與進貨模組（§8.8）相同的部分寫「同 §8.8」。邏輯在 `hammer_core::sales`（金額計算、訂單、出貨單、銷貨退回、建議批號），型別在 `hammer_shared::document`。

**銷貨以台幣交易**，三種單據都沒有幣別與匯率欄位；金額都是台幣。

#### 資料類別

M4c 的 migration 寫入下列 `entity_definitions`（`is_system = true`、`storage = table`、`is_active = true`、`icon` 為 `NULL`，id 為 migration 中固定的 UUID）：

| key | label | kind | table_name | 主表 | numbering_pattern | title_template | sort_order |
|---|---|---|---|---|---|---|---|
| `sales_order` | 訂單 | document | `sales_orders` | | `SO{yyyyMM}-{0000}` | `{record_no} {customer_id.name}` | 200 |
| `sales_order_line` | 訂單明細 | detail | `sales_order_lines` | `sales_order` | | `{product_id.code} {product_id.name}` | 0 |
| `shipment` | 出貨單 | document | `shipments` | | `SH{yyyyMM}-{0000}` | `{record_no} {customer_id.name}` | 210 |
| `shipment_line` | 出貨明細 | detail | `shipment_lines` | `shipment` | | `{product_id.code} {product_id.name} {lot_no}` | 0 |
| `sales_return` | 銷貨退回 | document | `sales_returns` | | `SR{yyyyMM}-{0000}` | `{record_no} {customer_id.name}` | 220 |
| `sales_return_line` | 銷貨退回明細 | detail | `sales_return_lines` | `sales_return` | | `{product_id.code} {product_id.name}` | 0 |

六張表的共同規則與 §8.8「資料類別」相同（§8.6「系統資料表的共同規則」、§7.1「系統單據」、`posted_at` / `posted_by`、`voided_at` / `voided_by` 與四個 CHECK、「系統維護」的欄位），差別如下：

- 索引：`(<單據日期>, id)`、`(status)`、`(customer_id)`；明細 `(<主表單數>_id, sort_order, id) WHERE deleted_at IS NULL`、`(product_id)`。
- 訂單的 `posted_at` / `posted_by` 記錄**確認**的時間與人員（同採購單）。
- 沒有幣別、匯率、`total_twd`。

#### `sales_orders`（訂單，系統單據 `sales_order`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| customer_id | UUID NOT NULL FK → customers | `relation` | 客戶 | 必填、→ `customer` |
| order_date | DATE NOT NULL | `date` | 訂單日期 | 必填。桌面程式新增時預設今天 |
| delivery_date | DATE | `date` | 預計交貨日 | |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 必填；選項與商品的 `tax_type` 相同；見「金額與稅額」 |
| tax_included | BOOLEAN NOT NULL DEFAULT false | `boolean` | 單價含稅 | 必填、預設 `false`；見「金額與稅額」 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft`（草稿）/ `confirmed`（已確認）/ `partially_shipped`（部分出貨）/ `closed`（已結案）/ `void`（已作廢） |
| subtotal | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 未稅金額 | 系統維護、小數 4 |
| tax_amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 稅額 | 系統維護、小數 4 |
| total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 總計 | 系統維護、小數 4 |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | 確認、作廢的時間與人員 |
| closed_at / closed_by | TIMESTAMPTZ / UUID FK → users | （不是欄位） | | **手動**結案的時間與人員；CHECK 同採購單 |

#### `sales_order_lines`（明細子表 `sales_order_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| sales_order_id | UUID NOT NULL FK → sales_orders | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| product_id | UUID NOT NULL FK → products | `relation` | 商品 | 必填、→ `product`；`goods`、`service` 都可以 |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 必填、小數 4、`min 0`；含稅或未稅依表頭的 `tax_included` |
| discount_rate | NUMERIC(6,4) NOT NULL DEFAULT 0 CHECK (discount_rate >= 0 AND discount_rate <= 1) | `decimal` | 折扣率 | 必填、預設 `0`、小數 4、`min 0`、`max 1`；減少的比例，`0.1` = 打九折 |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4；見「金額與稅額」 |
| shipped_qty | NUMERIC(18,4) NOT NULL DEFAULT 0 CHECK (shipped_qty >= 0) | `decimal` | 已出貨數量 | 系統維護、小數 4；出貨單過帳時增加、作廢時減少 |
| notes | TEXT | `long_text` | 備註 | |

#### `shipments`（出貨單，系統單據 `shipment`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| customer_id | UUID NOT NULL FK → customers | `relation` | 客戶 | 必填、→ `customer` |
| sales_order_id | UUID FK → sales_orders | `relation` | 訂單 | → `sales_order`；可以不經訂單直接出貨；見「訂單轉出貨單」 |
| ship_date | DATE NOT NULL | `date` | 出貨日期 | 必填；異動的 `txn_date`。桌面程式新增時預設今天 |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 必填 |
| tax_included | BOOLEAN NOT NULL DEFAULT false | `boolean` | 單價含稅 | 必填、預設 `false` |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft`（草稿）/ `posted`（已過帳）/ `void`（已作廢） |
| subtotal / tax_amount / total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 未稅金額 / 稅額 / 總計 | 系統維護、小數 4 |
| cost_total_twd | NUMERIC(18,2) | `money` | 銷貨成本 | 系統維護、小數 2；過帳時寫入 = 明細 `cost_amount_twd` 的合計（`service` 不計），草稿為 `NULL` |
| ship_address | TEXT | `text` | 送貨地址 | 桌面程式選擇客戶時帶入客戶的 `address`（可修改） |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(sales_order_id)`。

#### `shipment_lines`（明細子表 `shipment_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| shipment_id | UUID NOT NULL FK → shipments | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| sales_order_line_id | UUID FK → sales_order_lines | `relation` | 訂單明細 | → `sales_order_line`（系統明細子表，§7.3 的例外）、`is_visible = false`；必須屬於表頭的訂單 |
| product_id | UUID NOT NULL FK → products | `relation` | 商品 | 必填、→ `product`；`goods`、`service` 都可以 |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 必填、小數 4、`min 0` |
| discount_rate | NUMERIC(6,4) NOT NULL DEFAULT 0 CHECK (discount_rate >= 0 AND discount_rate <= 1) | `decimal` | 折扣率 | 同訂單明細 |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4 |
| lot_no | TEXT | `text` | 批號 | `max_length 50`；**手動指定批號**：留空 = 先進先出，有值 = 從這個批號出貨（見「出貨的批號」）；`service` 不可填 |
| cost_amount_twd | NUMERIC(18,2) | `money` | 成本 | 系統維護、小數 2；過帳時寫入 `round(Σ 批號扣除的 qty × unit_cost_twd, 2)`；`service` 與草稿為 `NULL` |
| notes | TEXT | `long_text` | 備註 | |
| lot_id | UUID FK → inventory_lots | （不是欄位） | | 手動指定批號時，過帳寫入該批號；先進先出（可能跨多個批號）與 `service` 為 `NULL`，扣了哪些批號以 `lot_allocations` 為準 |

另外有索引 `(sales_order_line_id)`、`(lot_id)`。

#### `sales_returns`（銷貨退回，系統單據 `sales_return`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| shipment_id | UUID NOT NULL FK → shipments | `relation` | 出貨單 | 必填、→ `shipment`；必須是已過帳的出貨單；新增後不可修改（`422`，「建立後不可更換出貨單」） |
| customer_id | UUID NOT NULL FK → customers | `relation` | 客戶 | 系統維護；= 出貨單的客戶 |
| return_date | DATE NOT NULL | `date` | 退回日期 | 必填；異動的 `txn_date`。桌面程式新增時預設今天；不可早於出貨單的 `ship_date`（`422`，「退回日期不可早於出貨日期」） |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 系統維護；= 出貨單的稅別 |
| tax_included | BOOLEAN NOT NULL | `boolean` | 單價含稅 | 系統維護；= 出貨單的值 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft` / `posted` / `void` |
| subtotal / tax_amount / total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 未稅金額 / 稅額 / 總計 | 系統維護、小數 4 |
| cost_total_twd | NUMERIC(18,2) | `money` | 退回成本 | 系統維護、小數 2；過帳時寫入，草稿為 `NULL` |
| reason | TEXT | `text` | 退回原因 | |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(shipment_id)`。

#### `sales_return_lines`（明細子表 `sales_return_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| sales_return_id | UUID NOT NULL FK → sales_returns | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| shipment_line_id | UUID NOT NULL FK → shipment_lines | `relation` | 出貨明細 | 必填、→ `shipment_line`（系統明細子表，§7.3 的例外）、`is_visible = false`；必須屬於表頭的出貨單 |
| product_id | UUID NOT NULL FK → products | `relation` | 商品 | 系統維護、→ `product`；= 出貨明細的商品 |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0，上限見「銷貨退回」 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 系統維護、小數 4；= 出貨明細的單價 |
| discount_rate | NUMERIC(6,4) NOT NULL DEFAULT 0 CHECK (discount_rate >= 0 AND discount_rate <= 1) | `decimal` | 折扣率 | 系統維護、小數 4；= 出貨明細的折扣率 |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4 |
| cost_amount_twd | NUMERIC(18,2) | `money` | 成本 | 系統維護、小數 2；過帳時寫入 `round(Σ 加回的 qty × unit_cost_twd, 2)`；`service` 與草稿為 `NULL` |
| notes | TEXT | `long_text` | 備註 | |

另外有索引 `(shipment_line_id)`。退回到哪些批號記錄在 `inventory_transactions`（`txn_type = sales_return`、`ref_line_id` 為退回明細），不另外建表（見「銷貨退回」）。

#### 金額與稅額

三種單據使用同一套計算（`hammer_core::sales::amounts`，`rust_decimal`，四捨五入一律 `MidpointAwayFromZero`；可與 `purchasing::amounts` 共用稅率與四捨五入的部分）。每次存檔時依明細重新計算並寫入系統維護的欄位；確認、過帳不重新計算。

- 小數位數 `d` = 台幣（TWD）在存檔當下的 `currencies.decimal_places`（預設 0）。
- **稅別以整張單據為單位**（表頭的 `tax_type`），稅額以整張計算（同 §8.8）。稅率取 `tax_rates.rate`。
- **單價含稅或未稅**由表頭的 `tax_included` 決定，同一張單據的所有明細相同；明細的 `amount` 與單價同一個基準（含稅單據的 `amount` 是含稅金額）。
- **折扣**以明細的 `discount_rate` 計算，第一版沒有整張單據的折扣；單價不可為負數，整張的折扣請分攤到各列的 `discount_rate`。

| 欄位 | 未稅（`tax_included = false`） | 含稅（`tax_included = true`） |
|---|---|---|
| 明細 `amount` | `round(qty × unit_price × (1 − discount_rate), d)` | 同左 |
| `total` | `subtotal + tax_amount` | `Σ amount` |
| `subtotal` | `Σ amount` | `round(total ÷ (1 + 稅率), d)` |
| `tax_amount` | `round(subtotal × 稅率, d)` | `total − subtotal` |

- 稅率為 0（`zero_rated`、`exempt`）時，含稅與未稅的結果相同（`subtotal = total`、`tax_amount = 0`）。
- **稅別的預設值**：新增時沒有帶 `tax_type` 時，明細的商品 `tax_type` 全部相同 → 帶入該值；否則（含沒有明細）→ `taxable`。管理者在欄位定義設定了 `default_value` 時，以 `default_value` 為準。出貨單有 `sales_order_id` 時帶入訂單的稅別；銷貨退回一律等於出貨單。
- **商品的 `tax_type`** 只用於上一點的預設值，不參與計算、不檢查是否與表頭相同（例如應稅商品外銷時開零稅率）。桌面程式在明細商品的稅別與表頭不同時顯示提示。
- **`default_sale_price`** 是**未稅**價。帶入是桌面程式的功能（伺服器不帶入，`unit_price` 必填）：選擇商品時，`tax_included = false` 帶入 `default_sale_price`，`true` 帶入 `round(default_sale_price × (1 + 稅率), 4)`；沒有設定時留空。之後修改 `tax_included` 或稅別不換算已輸入的單價。

**範例一：未稅、應稅**（`d = 0`、稅率 0.05）

| 明細 | qty | unit_price | discount_rate | amount |
|---|---|---|---|---|
| 1 | 3 | 33.3333 | 0 | round(99.9999, 0) = 100 |
| 2 | 10 | 120.0000 | 0.15 | round(1020.0000, 0) = 1020 |

`subtotal` = 1120；`tax_amount` = round(56.00, 0) = 56；`total` = 1176。

**範例二：含稅、應稅**（`d = 0`、稅率 0.05）

| 明細 | qty | unit_price | discount_rate | amount |
|---|---|---|---|---|
| 1 | 2 | 105.0000 | 0 | 210 |
| 2 | 5 | 99.0000 | 0.1 | round(445.5000, 0) = 446 |

`total` = 656；`subtotal` = round(656 ÷ 1.05, 0) = round(624.7619…, 0) = 625；`tax_amount` = 656 − 625 = 31。

**範例三：含稅、免稅**（稅率 0）：明細 `amount` 合計 500 → `total` = 500、`subtotal` = 500、`tax_amount` = 0。

#### 存檔時的檢查

違反時 `422 VALIDATION`，格式、關聯欄位的存在檢查、修改時沒有帶到明細的處理同 §8.8「存檔時的檢查」。

| 單據 | 檢查 | 錯誤位置與訊息 |
|---|---|---|
| 三種 | `qty > 0`、`unit_price ≥ 0`、`0 ≤ discount_rate ≤ 1` | `qty`「數量必須大於 0」；`unit_price`「單價不可小於 0」；`discount_rate`「折扣率必須介於 0 與 1 之間」 |
| 出貨單 | `goods`：`lot_no` 有值時必須是該商品未刪除的批號 | `lot_no`「找不到此批號」 |
| 出貨單 | `service`：`lot_no` 不可填 | `lot_no`「服務類商品沒有批號」 |
| 出貨單 | 有 `sales_order_id` 時：見「訂單轉出貨單」；沒有時明細不可有 `sales_order_line_id` | `sales_order_line_id`「沒有指定訂單」 |
| 銷貨退回 | 見「銷貨退回」 | |

- 存檔時**不檢查**庫存（過帳時檢查）；同一個批號可以出現在多列（過帳時合計檢查）。
- 客戶的 `credit_limit` **第一版不檢查**（只供參考）；應收帳款在 M6，之後再決定是否在確認訂單或出貨過帳時檢查。

#### 訂單的狀態

與採購單相同（§8.8「採購單的狀態」），`partially_received` 改為 `partially_shipped`、`received_qty` 改為 `shipped_qty`、進貨改為出貨：

```
draft ──確認──▶ confirmed ──出貨過帳──▶ partially_shipped ──全部出貨──▶ closed
  │                │  ▲                     │  ▲                        ▲
  │                │  └────出貨作廢──────────┘  └──────出貨作廢──────────┘（自動結案時）
  │                ├──手動結案──▶ closed ◀──手動結案──┘
  │                └──作廢──▶ void
  └── 可修改、刪除
```

| 動作 | 端點（§10「單據動作」） | 前一個狀態 → 之後 | 條件 | 寫入 |
|---|---|---|---|---|
| 確認 | `POST /api/v1/entities/sales_order/records/{id}/confirm` | `draft` → `confirmed` | 重新執行存檔時的檢查；至少一列明細（「至少要有一筆明細」）；客戶、明細的商品未刪除（「客戶已刪除」「商品已刪除」）。不檢查庫存 | `posted_at`、`posted_by`；操作紀錄 `confirm` |
| 手動結案 | `POST …/{id}/close` | `confirmed`、`partially_shipped` → `closed` | 無（不再出貨的部分視為取消） | `closed_at`、`closed_by`；操作紀錄 `close` |
| 作廢 | `POST …/{id}/void` | `confirmed` → `void` | 只有 `confirmed`（沒有任何有效的出貨）可以作廢；有出貨時請手動結案 | `voided_at`、`voided_by`；操作紀錄 `void` |
| 出貨過帳、出貨作廢 | （出貨單的動作） | 依「狀態的重新計算」 | | `shipped_qty` |

- 權限、request、檢查順序、`422 INVALID_STATUS`、不支援 `post`（`404`）、沒有「取消確認」與「取消結案」、**狀態的重新計算**（手動結案維持 `closed`；每一列 `shipped_qty ≥ qty` → 自動結案；有任何一列 `> 0` → `partially_shipped`；都是 0 → `confirmed`；有變動時 `version` + 1 並寫操作紀錄 `update`）都與採購單相同。
- **銷貨退回不影響**訂單的 `shipped_qty` 與狀態（同進貨退回）：要補出貨時另開出貨單（可以不指定訂單）。

#### 訂單轉出貨單

與「採購單轉進貨單」（§8.8）相同，由桌面程式帶入，不另外提供伺服器端點：

| 出貨單欄位 | 帶入的值 |
|---|---|
| `customer_id`、`tax_type`、`tax_included` | 訂單的值（伺服器在新增時沒有帶 `tax_type` 也會帶入訂單的稅別） |
| `ship_address` | 客戶的 `address` |
| 明細 | 訂單中**未出貨數量** `qty − shipped_qty > 0` 的每一列：`sales_order_line_id`、`product_id`、`unit_price`、`discount_rate`、`qty` = 未出貨數量；`lot_no` 留空（先進先出） |

**檢查**（存檔時與過帳時都檢查；過帳時在鎖定訂單後以最新的 `shipped_qty` 檢查）：

| 檢查 | 錯誤位置與訊息 |
|---|---|
| 訂單的狀態是 `confirmed` 或 `partially_shipped` | `sales_order_id`「訂單尚未確認」「訂單已結案」「訂單已作廢」 |
| 出貨單的 `customer_id` 與訂單相同 | `customer_id`「客戶與訂單不同」 |
| `sales_order_line_id` 是該訂單未刪除的明細 | `sales_order_line_id`「不是此訂單的明細」 |
| 明細的 `product_id` 與訂單明細相同 | `product_id`「商品與訂單明細不同」 |
| **不允許超出**：同一張出貨單中指向同一列訂單明細的 `qty` 合計 ≤ `qty − shipped_qty` | 第一列的 `qty`「超過未出貨數量 3.0000」 |

- `shipped_qty` 只計算已過帳的出貨單，草稿不保留數量（同 §8.8）。
- 同一列訂單明細可以拆成多列出貨（例如手動指定兩個批號），合計受上一點限制。
- 客戶要的比訂單多時，多出的部分以沒有 `sales_order_line_id` 的明細輸入。
- 稅別、含稅、單價、折扣率可以與訂單不同，不檢查。

#### 出貨的批號

D10：預設先進先出，可以手動指定批號。

- **一列明細最多指定一個批號**，存在明細的 `lot_no`（草稿階段的手動批號就是這個欄位，不另外建表）。留空 = 先進先出（過帳時可能跨多個批號）；有值 = 只從該批號出貨。
- 同一個商品要從多個批號出貨時，分成多列（各列的 `sales_order_line_id` 可以相同）。桌面程式提供「指定批號」：以「建議批號 API」取得先進先出的結果，使用者確認或修改後，把該列拆成每個批號一列並填入 `lot_no`。
- `lot_no` 去除前後空白後儲存，空字串視為留空。批號以 `product_id` + `lot_no`（不分大小寫）找到未刪除的批號，與盤點調整單的 `loss` 相同。

#### 建議批號 API

`POST /api/v1/inventory/allocation-preview`（需要 `record.read`；型別在 `hammer_shared::inventory`，邏輯與出貨過帳共用 `hammer_core::inventory` 的分配函式）。**只試算**：不鎖定批號、不寫入任何資料，不考慮其他草稿；實際扣除以過帳當下為準。

request：

```json
{ "lines": [
    { "product_id": "…", "qty": "12", "lot_no": null },
    { "product_id": "…", "qty": "3", "lot_no": "L-01" } ] }
```

- `lines` 1–200 列；`qty > 0`、最多 4 位小數；`lot_no` 可省略。格式錯誤 → `422 VALIDATION`，`details.lines` 依 §10「主表與明細」（`detail_key` 為 `null`）。
- 商品不存在、已刪除 → 該列的 `product_id`「商品不存在」；`lot_no` 找不到 → 該列的 `lot_no`「找不到此批號」。
- 分配順序與出貨過帳相同（見「出貨過帳」）：先扣有 `lot_no` 的列，再依陣列順序以先進先出扣其他列，同一商品的多列依序接著扣。

回應 `200`：

```json
{ "lines": [
    { "index": 0, "product_id": "…", "product_type": "goods", "qty": "12.0000",
      "allocations": [
        { "lot_id": "…", "lot_no": "L-01", "received_date": "2026-10-02",
          "qty": "5.0000", "unit_cost_twd": "80.0000" },
        { "lot_id": "…", "lot_no": "L-02", "received_date": "2026-11-01",
          "qty": "7.0000", "unit_cost_twd": "82.5000" } ],
      "shortage": "0.0000" },
    { "index": 1, "…": "…" } ] }
```

- `allocations` 依扣除順序；`shortage` 為不足的數量（足夠時 `"0.0000"`）。庫存不足**不是錯誤**，回傳能分配的部分與 `shortage`。
- `service` 商品：`allocations` 為空陣列、`shortage` 為 `"0.0000"`。

#### 出貨過帳與作廢

依 §9.2、§9.3、§10「單據動作」（`post`、`void`），鎖定依 §8.10「鎖定順序」：出貨單（`FOR UPDATE`）→ 訂單（有 `sales_order_id` 時，`FOR UPDATE`）→ 商品（`FOR SHARE`）→ 批號（`FOR UPDATE`；過帳為指定的批號 + 先進先出列的商品所有 `qty_remaining > 0` 的批號，作廢為 `lot_allocations` 的批號）→ 訂單明細（依 id `FOR UPDATE`）。

**過帳**：

1. 檢查 `version`、`status = draft`；重新執行存檔時的檢查（含「訂單轉出貨單」）、至少一列明細、客戶與商品未刪除（「客戶已刪除」「商品已刪除」）。
2. 依以下順序扣庫存（`goods` 才處理；先處理指定批號的列，避免先進先出先用掉使用者指定的批號）：
   1. 有 `lot_no` 的列，依 `sort_order`：`qty ≤ qty_remaining`（同一批號出現在多列時依序扣），不足時記下短缺並繼續；足夠時扣除。寫入明細的 `lot_id`。
   2. 沒有 `lot_no` 的列，依 `sort_order`：依先進先出順序（`received_date, id`，§8.10）從 `qty_remaining > 0` 的批號扣，直到扣滿；不足時記下短缺（一個商品一項，§10）並繼續。
   - 每一次從批號扣除：新增 `lot_allocations`（`source_type = shipment_line`、批號的成本）、`qty_remaining −= qty`、新增異動（`shipment`、`−qty`、`txn_date = ship_date`、`ref_doc_type = shipment`）。
3. 有短缺 → rollback，`422 STOCK_INSUFFICIENT`，列出所有短缺。
4. 每一列寫入 `cost_amount_twd`（`service` 為 `NULL`）；有 `sales_order_line_id`：訂單明細的 `shipped_qty += qty`；明細有變動時 `version` + 1。
5. 重新計算訂單的狀態；`cost_total_twd` = Σ 明細的 `cost_amount_twd`。
6. `status = posted`、`posted_at`、`posted_by`，`version` + 1；寫入操作紀錄（`post`）。

**作廢**：

1. 檢查 `version`、`status = posted`。
2. 沒有**有效的銷貨退回**（指向此出貨單、`status = posted` 的 `sales_returns`）→ 否則 `422 VALIDATION`，`details` 與進貨單相同（`{ "fields": [], "documents": [ … ] }`），訊息「此出貨單已有銷貨退回 SR202611-0002，請先作廢退回單」。
3. 依各列的 `lot_allocations` 把數量加回原批號，新增異動（`void_reversal`、`+qty`、扣除時的成本、`txn_date` 為作廢當天）。原批號不會已被刪除（批號有出貨的有效異動時，產生它的單據不可作廢，§8.10）。
4. 有 `sales_order_line_id` 的每一列：`shipped_qty −= qty`；重新計算訂單的狀態。
5. `status = void`、`voided_at`、`voided_by`，`version` + 1；寫入操作紀錄（`void`）。`cost_amount_twd`、`cost_total_twd` 保留（紀錄）。

- 被有效的銷項發票關聯時不可作廢（在步驟 2 之後檢查，§8.11「其他單據的作廢限制」）：`422 VALIDATION`，訊息「此出貨單已開立發票 AB12345678，請先作廢發票」。

#### 銷貨退回

退回**已過帳**出貨單的部分或全部明細。建立方式與進貨退回相同由桌面程式帶入：選擇出貨單後，列出每一列出貨明細的**可退數量**，使用者選擇要退的列與數量。

- 存檔與過帳時檢查出貨單 `status = posted`（`shipment_id`「出貨單尚未過帳」「出貨單已作廢」）。
- 表頭的 `customer_id`、`tax_type`、`tax_included`，明細的 `product_id`、`unit_price`、`discount_rate` 由伺服器在存檔時從出貨單、出貨明細帶入（系統維護）。金額依「金額與稅額」計算。
- 明細的 `shipment_line_id` 必須是表頭出貨單未刪除的明細（「不是此出貨單的明細」）；同一張退回單中同一列出貨明細只能出現一次（「出貨明細重複」）。

**退回數量上限**（存檔時與過帳時都檢查；過帳時在鎖定出貨單後檢查）：

```
可退數量 = 出貨明細的 qty − 該出貨明細在其他「已過帳」銷貨退回中的 qty 合計
```

超過 → `422 VALIDATION`，`qty`「超過可退數量 4.0000」。草稿不保留數量。

**批號的分配**：退回的數量加回**原出貨扣除的批號**，成本用原批號的成本（`lot_allocations.unit_cost_twd`）。

- 一列出貨明細有多筆 `lot_allocations`（先進先出跨批號）時，依**扣除的反向順序**（批號的 `received_date DESC, id DESC`，即最後扣的先加回）分配。
- 每個批號可加回的數量 = 該出貨明細從此批號扣除的 `qty` − 此出貨明細的其他已過帳退回加回此批號的數量（由 `inventory_transactions` 中 `txn_type = sales_return`、`ref_line_id` 為這些退回明細、該批號的 `qty_change` 合計計算）。依上一點的順序填滿，直到退回的數量分配完。退回數量上限保證一定分配得完。
- 範例：出貨 12（L-01 扣 5、L-02 扣 7）。第一次退 8 → L-02 加回 7、L-01 加回 1；第二次退 4 → L-01 加回 4。

**過帳**（鎖定順序：退回單 → 出貨單（`FOR UPDATE`，同一張出貨單的退回依序過帳）→ 商品（`FOR SHARE`）→ 原出貨的批號（`FOR UPDATE`））：

1. 檢查 `version`、`status = draft`；重新執行存檔時的檢查、至少一列明細、退回數量上限。
2. 依 `sort_order` 處理每一列：
   - `goods`：依上方分配，每個批號 `qty_remaining += qty`、新增異動（`sales_return`、`+qty`、原批號的成本、`txn_date = return_date`、`ref_doc_type = sales_return`）；`cost_amount_twd = round(Σ qty × unit_cost_twd, 2)`。
   - `service`：不處理庫存；`cost_amount_twd` 為 `NULL`。
3. `cost_total_twd` = Σ 明細的 `cost_amount_twd`；`status = posted`、`posted_at`、`posted_by`，`version` + 1；寫入操作紀錄（`post`）。

- 加回不會不足，所以過帳不會有 `STOCK_INSUFFICIENT`。原批號不會已被刪除（同出貨作廢）。
- 退回的金額（`total`）只是紀錄，應收帳款的沖銷在 M6；發票折讓見 §8.11「銷貨折讓」。

**作廢**：被有效的銷貨折讓關聯時不可作廢（§8.11「其他單據的作廢限制」：「此銷貨退回已有折讓單 SA202611-0001，請先作廢折讓單」）。鎖定順序同過帳（批號為此退回單的 `sales_return` 異動的批號）。依此退回單的每一筆 `sales_return` 異動：該批號 `qty_remaining ≥ qty`，不足時記下短缺（加回的貨已再出貨或盤虧）；足夠時 `qty_remaining −= qty`、新增異動（`void_reversal`、`−qty`、同一個成本、`txn_date` 為作廢當天）。有短缺 → rollback，`422 STOCK_INSUFFICIENT`（指定批號的格式，一個批號一項）。`cost_amount_twd`、`cost_total_twd` 保留。

#### 客戶、商品被銷貨單據引用時的限制

`hammer_core::master_data`，補充 §7.2，規則與 §8.8「廠商、商品被進貨單據引用時的限制」相同：

| 資料 | 操作 | 規則 |
|---|---|---|
| 客戶 | 刪除 | 有任何未刪除的訂單、出貨單、銷貨退回（**不論狀態**，含草稿與已作廢）指向它時不可刪除：`422 VALIDATION`，`details` 為 `{ "fields": [], "document_count": 3 }`，訊息「此客戶已有 3 張訂單或出貨單據，無法刪除，請改為停用」 |
| 商品 | 刪除、`product_type` 由 `goods` 改為 `service`、修改 `unit_id` | 除了 §7.2 的庫存異動、§8.8 的未結案採購單，另外在有**未結案的訂單**（`confirmed`、`partially_shipped`，未刪除）的明細引用時不可：刪除的 `details` 為 `{ "fields": [], "open_sales_order_count": 2 }`，訊息「此商品有 2 張未結案的訂單，無法刪除」；修改時 `details.fields` 指出該欄位。同時有未結案的採購單時 `details` 兩個數量都列出，訊息為第一個 |

- 鎖定、草稿與已結案訂單不限制、停用不受限制，都與 §8.8 相同。
- 被銷項發票、銷貨折讓引用時的限制見 §8.11「客戶、廠商被引用時的限制」（廠商被進項憑證、進貨折讓引用時同）。
- 信用額度不檢查（見「存檔時的檢查」）。

---

### 8.10 庫存模組

> M4a 定案。邏輯在 `hammer_core::inventory`（庫存）、`hammer_core::stock_adjustments`（盤點與調整單），型別在 `hammer_shared::inventory`、`hammer_shared::document`。

**共同規則**：

- 只有 `product_type = goods` 的商品有批號與庫存異動；`service` 的商品不產生批號、批號扣除、異動，也不出現在庫存查詢。單據過帳時，`service` 商品的明細一律跳過庫存處理。盤點調整單不可使用 `service` 商品（見下方存檔時的檢查）；採購單、進貨單、進貨退回、訂單、出貨單、銷貨退回可以有 `service` 商品的明細（例如運費，§8.8、§8.9）。
- 三張表都不是資料類別：沒有自訂欄位、版面，不使用通用資料 API，只能透過單據的過帳、作廢寫入，透過下方「庫存查詢 API」讀取。
- 數量、單位成本為 `NUMERIC(18,4)`，API 中為補齊 4 位小數的字串（§7.3）；金額（`stock_value_twd`）為 `NUMERIC(18,2)` 的字串。計算一律用 `rust_decimal`，四捨五入使用 `MidpointAwayFromZero`（與公式的 `ROUND` 相同）。
- `source_type`、`ref_doc_type` 等「來源類型」的值為資料類別代碼（單據的主表或明細子表代碼）。M4a 的 migration 只列出盤點調整單的代碼，M4b、M4c 的 migration 修改 `CHECK` 加上各自的代碼。

#### `inventory_lots`（批號）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PRIMARY KEY | |
| product_id | UUID NOT NULL FK → products | 必須是 `goods` |
| lot_no | TEXT NOT NULL | 批號；規則見下方「批號」 |
| received_date | DATE NOT NULL | 入庫日期 = 來源單據的單據日期（進貨日、調整日期）；先進先出的排序依據 |
| source_type | TEXT NOT NULL CHECK | 產生此批號的單據明細子表代碼：`stock_adjustment_line`、`goods_receipt_line`（M4b） |
| source_doc_id | UUID NOT NULL | 來源單據（主表）的 id |
| source_line_id | UUID NOT NULL | 來源明細的 id；UNIQUE (source_type, source_line_id)，一列明細最多產生一個批號 |
| qty_in | NUMERIC(18,4) NOT NULL CHECK (qty_in > 0) | 原始入庫數量，建立後不變 |
| qty_remaining | NUMERIC(18,4) NOT NULL CHECK (qty_remaining >= 0) | 目前數量；可能大於 `qty_in`（盤盈加回、銷貨退回） |
| unit_cost_twd | NUMERIC(18,4) NOT NULL CHECK (unit_cost_twd >= 0) | 台幣單位成本，建立後不變 |
| ＋共通欄位 | | `version` 在每次異動時 + 1，但不用於檢查（一律以 `FOR UPDATE` 鎖定後修改） |

- 索引：
  - 唯一：`(product_id, lower(lot_no)) WHERE deleted_at IS NULL`（同一商品的批號不分大小寫不可重複；批號被刪除後可以再使用）
  - 先進先出：`(product_id, received_date, id) WHERE qty_remaining > 0 AND deleted_at IS NULL`
  - `(source_doc_id)`；`set_updated_at` trigger
- 軟刪除只發生在作廢產生它的單據時（見下方「作廢」）；刪除時 `qty_remaining` 一併設為 0（`CHECK (deleted_at IS NULL OR qty_remaining = 0)`）。
- `CHECK (char_length(lot_no) BETWEEN 1 AND 50)`；去除空白、控制字元由 `hammer_core` 檢查。
- 先進先出的順序：`received_date, id`（同一天依建立順序，uuid v7）。

**批號**（`lot_no`）：

- 去除前後空白後儲存；不可有控制字元，最長 50 字元。
- 由使用者在單據明細輸入（盤點調整單的 `lot_no`、M4b 進貨單的 `lot_no`）。去除空白後是空字串時，**過帳時**自動產生：`{單號}-{明細序號}`，明細序號是該列在單據明細中的位置（`sort_order + 1`），至少 2 位、不足補零，例如 `ADJ202611-0003-01`、`ADJ202611-0003-12`。自動產生的批號寫回明細的 `lot_no`。
- 單號不可重複，所以自動產生的批號不會彼此重複；與使用者先前手動輸入的批號重複時，與手動輸入的重複相同處理（下一點）。
- 重複的檢查：存檔時檢查（與同商品未刪除的批號、與同一張單據的其他明細）；過帳時由唯一索引再擋一次（存檔後才建立的批號）。重複 → `422 VALIDATION`，明細的 `lot_no`，訊息「批號已存在」。
- 建立後不可修改。

#### `lot_allocations`（批號扣除明細）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PRIMARY KEY | |
| source_type | TEXT NOT NULL CHECK | 扣除庫存的單據明細子表代碼：`stock_adjustment_line`、`purchase_return_line`（M4b）、`shipment_line`（M4c） |
| source_doc_id | UUID NOT NULL | 單據（主表）的 id |
| source_line_id | UUID NOT NULL | 明細的 id |
| lot_id | UUID NOT NULL FK → inventory_lots | |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | 從該批號扣除的數量 |
| unit_cost_twd | NUMERIC(18,4) NOT NULL CHECK (unit_cost_twd >= 0) | 扣除當下該批號的成本（= 批號的 `unit_cost_twd`） |
| created_at | TIMESTAMPTZ NOT NULL DEFAULT now() | |
| created_by | UUID FK → users | |

- 索引：`(source_type, source_line_id)`、`(lot_id)`。
- 只新增，不修改也不刪除：資料庫以 trigger（`reject_append_only_change`）拒絕 `UPDATE`、`DELETE`、`TRUNCATE`（SQLSTATE `23001`）。作廢單據時依它把數量加回原批號，紀錄保留（成本的歷史）。
- 一列明細可以有多筆（先進先出跨多個批號）。

#### `inventory_transactions`（庫存異動流水帳）
| 欄位 | 型態 | 說明 |
|---|---|---|
| id | UUID PRIMARY KEY | |
| txn_at | TIMESTAMPTZ NOT NULL DEFAULT now() | 過帳或作廢的時間 |
| txn_date | DATE NOT NULL | 業務日期：過帳時為單據日期；作廢時為作廢當天（Asia/Taipei） |
| txn_type | TEXT NOT NULL CHECK | `receipt` / `shipment` / `sales_return` / `purchase_return` / `adjustment` / `void_reversal`（M4a 起就列出全部值） |
| product_id | UUID NOT NULL FK → products | |
| lot_id | UUID NOT NULL FK → inventory_lots | 每一筆異動都屬於一個批號 |
| qty_change | NUMERIC(18,4) NOT NULL CHECK (qty_change <> 0) | 入庫為正、出庫為負 |
| unit_cost_twd | NUMERIC(18,4) NOT NULL CHECK (unit_cost_twd >= 0) | 該批號的成本 |
| ref_doc_type | TEXT NOT NULL CHECK | 來源單據的主表代碼：`stock_adjustment`、`goods_receipt`、`purchase_return`（M4b）、`shipment`、`sales_return`（M4c） |
| ref_doc_id | UUID NOT NULL | 來源單據 id |
| ref_line_id | UUID NOT NULL | 來源明細 id |
| created_by | UUID FK → users | |

- 索引：`(product_id, txn_date, id)`、`(lot_id, ref_doc_type, ref_doc_id)`、`(ref_doc_type, ref_doc_id)`。
- 只新增，不修改也不刪除（沒有共通欄位）：與 `lot_allocations` 相同，以 trigger 拒絕 `UPDATE`、`DELETE`、`TRUNCATE`。作廢單據時新增 `txn_type = void_reversal` 的反向異動，`ref_*` 仍指向被作廢的單據與明細。
- 一張單據的作廢是全有或全無，所以「某張單據對某個批號的異動是否還有效」= 該單據對該批號的異動中**沒有** `void_reversal`。

#### `stock_adjustments`（盤點與調整單，系統單據 `stock_adjustment`）

依照 §7.1「系統單據」與 §8.6「系統資料表的共同規則」（欄位順序、`custom_data`、`formula_errors`、共通欄位、trigger、`field_definitions` 的寫法都相同）。用途：盤點差異、報廢等一般調整、**期初庫存**。

M4a 的 migration 寫入下列 `entity_definitions`（`is_system = true`、`storage = table`、`is_active = true`、`icon` 為 `NULL`，id 為 migration 中固定的 UUID）：

| key | label | kind | table_name | 主表 | numbering_pattern | title_template | sort_order |
|---|---|---|---|---|---|---|---|
| `stock_adjustment` | 盤點調整單 | document | `stock_adjustments` | | `ADJ{yyyyMM}-{0000}` | `{record_no}` | 100 |
| `stock_adjustment_line` | 盤點調整明細 | detail | `stock_adjustment_lines` | `stock_adjustment` | | `{product_id.code} {product_id.name} {lot_no}` | 0 |

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1「系統單據」；唯一索引 `(record_no)`（含已刪除） |
| adjustment_date | DATE NOT NULL | `date` | 調整日期 | 必填；新批號的 `received_date`、異動的 `txn_date`。桌面程式新增時預設今天 |
| adjustment_type | TEXT NOT NULL CHECK | `select` | 類型 | 必填、預設 `count`；選項 `opening`（期初庫存）/ `count`（盤點）/ `adjustment`（一般調整） |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護（§7.1）；選項 `draft`（草稿）/ `posted`（已過帳）/ `void`（已作廢） |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by | TIMESTAMPTZ / UUID FK → users | （不是欄位） | | 過帳時間與人員 |
| voided_at / voided_by | TIMESTAMPTZ / UUID FK → users | （不是欄位） | | 作廢時間與人員 |

- `CHECK ((status = 'draft') = (posted_at IS NULL))`、`CHECK ((status = 'void') = (voided_at IS NOT NULL))`、`CHECK ((posted_at IS NULL) = (posted_by IS NULL))`、`CHECK ((voided_at IS NULL) = (voided_by IS NULL))`。
- 索引：`(adjustment_date, id)`、`(status)`。

#### `stock_adjustment_lines`（明細子表 `stock_adjustment_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| stock_adjustment_id | UUID NOT NULL FK → stock_adjustments | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| product_id | UUID NOT NULL FK → products | `relation` | 商品 | 必填、→ `product`；必須是 `goods` |
| line_type | TEXT NOT NULL CHECK | `select` | 調整方式 | 必填、預設 `gain_new`；選項 `gain_new`（盤盈：新批號）/ `gain_existing`（盤盈：加回既有批號）/ `loss`（盤虧：扣指定批號） |
| lot_no | TEXT | `text` | 批號 | `max_length 50`；`gain_new` 可留空（過帳時自動產生），`gain_existing`、`loss` 必填 |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0（`hammer_core` 另外檢查，回傳 422）；一律為正數，增減由 `line_type` 決定 |
| unit_cost_twd | NUMERIC(18,4) CHECK (unit_cost_twd >= 0) | `money` | 單位成本 | 小數 4、`min 0`；`gain_new` 必填；`gain_existing`、`loss` 不可填，過帳時由系統寫入該批號的成本 |
| reason | TEXT | `text` | 原因 | 例如 盤點差異、報廢 |
| lot_id | UUID FK → inventory_lots | （不是欄位） | | 過帳時寫入：`gain_new` 為新建的批號，其他為指定的批號 |

- 索引：`(stock_adjustment_id, sort_order, id) WHERE deleted_at IS NULL`、`(product_id)`、`(lot_id)`。

**存檔時的檢查**（草稿的新增與修改；`hammer_core::stock_adjustments`，違反時 `422 VALIDATION`，明細錯誤依 §10「主表與明細」的格式）：

| 檢查 | 錯誤位置與訊息 |
|---|---|
| 商品是 `goods` | `product_id`「服務類商品沒有庫存」 |
| `qty > 0` | `qty`「數量必須大於 0」 |
| `gain_new`：`unit_cost_twd` 有值；`lot_no` 有值時不可與該商品未刪除的批號重複 | `unit_cost_twd`「必填」；`lot_no`「批號已存在」 |
| `gain_existing`、`loss`：`lot_no` 必填，必須是該商品未刪除的批號；`unit_cost_twd` 不可填 | `lot_no`「找不到此批號」；`unit_cost_twd`「既有批號的成本不可修改」 |
| 同一張單據中，同一商品的同一批號（不分大小寫）只能出現一次（`gain_new` 留空的批號不比對） | 第二列的 `lot_no`「批號重複」 |
| `adjustment_type = opening` 時只能使用 `gain_new` | `line_type`「期初庫存只能新增批號」 |

- 存檔時**不檢查**庫存是否足夠（過帳時檢查），所以草稿可以先輸入盤虧。
- 修改時沒有帶到明細，只在 `adjustment_type` 有變動時以既有的明細檢查（例如改為期初庫存）；其他修改不重新檢查明細（過帳時一律重新檢查）。
- 系統維護的欄位、`lot_id` 由伺服器寫入；明細 request 帶到 `lot_id` 時與其他未定義的欄位相同（`422`）。

**過帳**（`POST /api/v1/entities/stock_adjustment/records/{id}/post`，§10「單據動作」）：

1. 依「鎖定順序」鎖定單據（`FOR UPDATE`）；檢查 `version`、`status = draft`。
2. 依目前的資料重新執行全部存檔時的檢查（商品可能在存檔後被刪除或改類型、批號可能已被作廢刪除），並檢查至少有一列明細（`details.lines` 的 `index`、`field_key` 為 `null`，訊息「至少要有一筆明細」）；商品已刪除時為 `product_id`「商品已刪除」。
3. 鎖定商品（`FOR SHARE`）、指定的批號（`FOR UPDATE`）。
4. 依 `sort_order` 處理每一列：
   - `gain_new`：`lot_no` 留空時自動產生；新增 `inventory_lots`（`qty_in = qty_remaining = qty`、`unit_cost_twd` 為明細的成本、`received_date = adjustment_date`）；新增異動（`adjustment`、`+qty`）。
   - `gain_existing`：該批號 `qty_remaining += qty`；新增異動（`adjustment`、`+qty`、批號的成本）；明細的 `unit_cost_twd` 寫入批號的成本。
   - `loss`：`qty ≤ qty_remaining`，不足時記下短缺並繼續檢查其他列；足夠時新增 `lot_allocations`、`qty_remaining -= qty`、新增異動（`adjustment`、`−qty`）；明細的 `unit_cost_twd` 寫入批號的成本。
   - 寫入明細的 `lot_id`；明細有變動（`lot_no`、`unit_cost_twd`、`lot_id`）時明細的 `version` + 1。
5. 有任何短缺 → 整張單據 rollback，回傳 `422 STOCK_INSUFFICIENT`，列出**所有**短缺（§10）。
6. `status = posted`、`posted_at`、`posted_by`，`version` + 1；寫入操作紀錄（`post`）。

**作廢**（`POST /api/v1/entities/stock_adjustment/records/{id}/void`）：

1. 鎖定單據；檢查 `version`、`status = posted`。
2. 鎖定商品（`FOR SHARE`）、各明細的 `lot_id`（`FOR UPDATE`）。
3. 依 `sort_order` 處理每一列，異動一律為 `void_reversal`、`txn_date` 為作廢當天：
   - `gain_new`：該批號必須沒有**其他單據的有效異動**（`inventory_transactions` 中 `lot_id` 相同、來源是其他單據、且該單據對此批號沒有 `void_reversal` 的異動），否則記下並繼續檢查；可以時新增異動（`−qty_remaining`，此時等於 `qty`）、批號軟刪除。
   - `gain_existing`：該批號 `qty_remaining ≥ qty`，不足時記下短缺；足夠時 `qty_remaining -= qty`、新增異動（`−qty`）。
   - `loss`：依該列的 `lot_allocations` 把數量加回原批號，新增異動（`+qty`、扣除時的成本）。
4. 有批號被其他單據使用 → rollback，`422 LOT_IN_USE`；有短缺 → rollback，`422 STOCK_INSUFFICIENT`（兩者都有時回傳 `LOT_IN_USE`）。
5. `status = void`、`voided_at`、`voided_by`，`version` + 1；寫入操作紀錄（`void`）。作廢後不可再過帳，要重做時另開新單據。

**期初庫存**：開始使用系統時，以 `adjustment_type = opening` 的盤點調整單輸入，每一個批號一列（`gain_new`，填入批號、數量、台幣單位成本）。不限制只能有一張、也不限制日期；調整日期建議填開始使用系統的前一天，讓期初批號在先進先出中排在最前面。

**鎖定順序**（所有會鎖定庫存的操作都遵守，避免並行過帳 deadlock；`hammer_core::inventory` 提供共用的鎖定函式）：

1. 單據主表（`FOR UPDATE`）。同一個操作需要鎖多張單據時（例如 M4b 進貨過帳同時更新採購單），先鎖本張單據，其他單據依 `id` 由小到大。
2. 商品：單據所有明細的 `product_id` 去除重複後依 `id` 由小到大 `FOR SHARE`（避免過帳期間商品被刪除或改類型、單位，見 §7.2）。
3. 批號：一次查詢鎖定，`ORDER BY product_id, id FOR UPDATE`。範圍為指定的批號（`lot_id`、或依 `product_id` + `lot_no` 找到的批號），加上先進先出模式的商品所有 `qty_remaining > 0` 且未刪除的批號。
4. 其他單據的明細（例如採購單明細的 `received_qty`），依 `id` 由小到大。
5. 之後才新增 `inventory_lots`、`lot_allocations`、`inventory_transactions`（新增不會與其他 transaction 互相等待；批號重複由唯一索引擋下）。

- 鎖定後才讀取 `qty_remaining` 與做先進先出計算，不使用鎖定前讀到的值。
- 商品的刪除與修改先以 `FOR UPDATE` 鎖定商品再檢查庫存（§7.2），與過帳的 `FOR SHARE` 互斥，所以檢查後不會再有新的異動。

#### `v_stock_balances`（檢視表）
```sql
SELECT product_id,
       SUM(qty_remaining)                 AS qty_on_hand,
       SUM(qty_remaining * unit_cost_twd) AS stock_value_twd
FROM inventory_lots
WHERE deleted_at IS NULL
GROUP BY product_id;
```

- 只包含有批號的商品；沒有批號的 `goods` 商品在查詢 API 中庫存為 0。
- `stock_value_twd` 在 API 回傳時四捨五入到 2 位小數。

#### 庫存查詢 API

都需要 `record.read`；分頁、錯誤格式依 §10。商品不存在或已刪除 → `404`。邏輯在 `hammer_core::inventory_queries`，型別在 `hammer_shared::inventory`。

三個端點的篩選參數格式錯誤（uuid、日期、`true` / `false`、`txn_type` 不在選項中）一律 `422 VALIDATION`，`details.field` 為參數名稱；空字串視為未指定；`page`、`page_size` 不是整數時與其他列表相同為 `400`。

**商品庫存** `GET /api/v1/inventory/balances`

| 參數 | 說明 |
|---|---|
| `q` | 料號或品名包含（不分大小寫；以空白分隔的每個詞都要出現在料號或品名中） |
| `category_id` | 商品分類（只比對該分類本身，不含下層分類） |
| `in_stock` | `true`：只列出 `qty_on_hand > 0`；`false`：只列出庫存為 0；省略：全部 |
| `below_safety_stock` | `true`：只列出有設定 `safety_stock` 且 `qty_on_hand < safety_stock`；`false` 或省略：全部 |
| `include_inactive` | 預設 `true`（停用的商品可能還有庫存）；`false` 時排除 `is_active = false` |
| `page`、`page_size` | §10 |

- 範圍：未刪除、`product_type = goods` 的商品（`products LEFT JOIN v_stock_balances`）。排序：`lower(code), id`。
- 項目：`{ "product_id", "code", "name", "spec", "unit_name", "is_active", "qty_on_hand": "12.0000", "stock_value_twd": "3450.00", "safety_stock": "10.0000" | null, "below_safety_stock": false }`。

**商品的批號** `GET /api/v1/inventory/products/{product_id}/lots`

| 參數 | 說明 |
|---|---|
| `include_empty` | 預設 `false`：只列出 `qty_remaining > 0`；`true`：含已用完的批號 |
| `q` | 批號包含（不分大小寫） |
| `page`、`page_size` | §10 |

- 範圍：該商品未刪除的批號；依先進先出順序（`received_date, id`）排序。商品是 `service` 時回傳空列表。
- 項目：`{ "id", "lot_no", "received_date", "source_type", "source_doc_id", "source_record_no", "qty_in", "qty_remaining", "unit_cost_twd" }`（`source_record_no` 為來源單據的單號）。
- 桌面程式的批號選擇（盤點調整單的 `lot_no`、出貨單的 `lot_no`）使用此端點；出貨的先進先出試算另有「建議批號 API」（§8.9）。

**異動流水帳** `GET /api/v1/inventory/transactions`

| 參數 | 說明 |
|---|---|
| `product_id` | 商品 |
| `lot_id` | 批號 |
| `date_from`、`date_to` | `txn_date` 區間，含開始、含結束（與匯率列表相同） |
| `txn_type` | 異動類型 |
| `ref_doc_type`、`ref_doc_id` | 來源單據 |
| `page`、`page_size` | §10 |

- 排序：`txn_date DESC, id DESC`（新到舊）。
- 項目：`{ "id", "txn_at", "txn_date", "txn_type", "product_id", "product_code", "product_name", "lot_id", "lot_no", "qty_change", "unit_cost_twd", "ref_doc_type", "ref_doc_id", "ref_record_no", "ref_line_id", "created_by", "created_by_name" }`。
- 不回傳累計餘額（分頁下無法正確計算）；需要時另以商品庫存查詢目前數量。
- 參數格式錯誤（日期、uuid、`txn_type` 不在選項中）→ `422 VALIDATION`，`details.field` 為參數名稱。

---

### 8.11 發票模組

> M5 定案（Q1，2026-11-19）。**只做內部紀錄**：發票在別處開立（手開或廠商開立），系統只登錄號碼與金額；不上傳財政部、不串接加值中心、**不做字軌配號**；401 申報媒體檔在 M7 再評估，應收應付沖帳在 M6。單號、明細、狀態限制、過帳與作廢的共通規則以 §7.1「系統單據」、§10「單據動作」為準。邏輯在 `hammer_core::invoicing`（金額、號碼、期別、銷項發票、銷貨折讓、進項憑證、進貨折讓），型別在 `hammer_shared::invoice`。

**四種單據**（狀態都是 `draft` → `posted` → `void`，§8.2；畫面上 `posted` 顯示為「已開立」，進項憑證顯示為「已登錄」）：

| 單據 | 用途 | 明細 | 關聯 |
|---|---|---|---|
| 銷項發票 | 手開的三聯式、二聯式發票 | 品名明細 | 0–多張已過帳出貨單（同客戶） |
| 銷貨折讓 | 銷貨折讓證明單 | 品名明細 | 一張已開立的銷項發票；可選一張銷貨退回 |
| 進項憑證 | 廠商開立的三聯式 / 電子發票、海關代徵營業稅繳納證 | **沒有品名明細**，只記表頭金額 | 0–多張已過帳進貨單（同廠商） |
| 進貨折讓 | 對廠商的進貨退出或折讓證明單 | **沒有品名明細**，只記表頭金額 | 一張已登錄的進項憑證；可選一張進貨退回 |

**金額一律是台幣**（沒有幣別、匯率欄位）。外幣進貨的進項憑證以憑證上的台幣金額登錄（海關繳納證本來就是台幣），與進貨單的外幣金額不比對。

#### 資料類別

M5 的 migration 寫入下列 `entity_definitions`（`is_system = true`、`storage = table`、`is_active = true`、`icon` 為 `NULL`，id 為 migration 中固定的 UUID）：

| key | label | kind | table_name | 主表 | numbering_pattern | title_template | sort_order |
|---|---|---|---|---|---|---|---|
| `sales_invoice` | 銷項發票 | document | `sales_invoices` | | `SI{yyyyMM}-{0000}` | `{invoice_no} {buyer_name}` | 300 |
| `sales_invoice_line` | 發票明細 | detail | `sales_invoice_lines` | `sales_invoice` | | `{description}` | 0 |
| `sales_invoice_shipment` | 發票出貨單 | detail | `sales_invoice_shipments` | `sales_invoice` | | `{shipment_id.record_no}` | 0 |
| `sales_allowance` | 銷貨折讓 | document | `sales_allowances` | | `SA{yyyyMM}-{0000}` | `{record_no} {customer_id.name}` | 310 |
| `sales_allowance_line` | 折讓明細 | detail | `sales_allowance_lines` | `sales_allowance` | | `{description}` | 0 |
| `purchase_invoice` | 進項憑證 | document | `purchase_invoices` | | `PI{yyyyMM}-{0000}` | `{invoice_no} {supplier_id.name}` | 320 |
| `purchase_invoice_receipt` | 憑證進貨單 | detail | `purchase_invoice_receipts` | `purchase_invoice` | | `{goods_receipt_id.record_no}` | 0 |
| `purchase_allowance` | 進貨折讓 | document | `purchase_allowances` | | `PA{yyyyMM}-{0000}` | `{record_no} {supplier_id.name}` | 330 |

- 系統單號 `record_no`（`SI202611-0001`，依 §7.1 取號）與**發票號碼** `invoice_no`（`AB12345678`，使用者輸入）是兩個欄位；選擇器與標題顯示發票號碼。
- 關聯出貨單、進貨單以**明細子表**（`sales_invoice_shipment`、`purchase_invoice_receipt`）記錄，與其他明細一樣以 `lines.<明細子表代碼>` 整組取代（§10「主表與明細」），不另外提供端點。
- 八張表的共同規則與 §8.8「資料類別」相同（§8.6「系統資料表的共同規則」、§7.1「系統單據」、`posted_at` / `posted_by`、`voided_at` / `voided_by` 與四個 CHECK、「系統維護」的欄位）。另外：
  - 四張主表都有 `void_date` / `void_reason`（作廢日期、原因，系統維護的欄位，由作廢端點寫入，見「開立與作廢」），`CHECK (status <> 'draft' OR void_date IS NULL)`、`CHECK (status <> 'void' OR void_date IS NOT NULL)`、`CHECK ((void_date IS NULL) = (void_reason IS NULL))`（作廢時先寫入作廢日期與原因，框架再把狀態改為 `void`，同一個 transaction 內；0015 起）。
  - 索引：`(<單據日期>, id)`、`(status)`、`(customer_id)` 或 `(supplier_id)`；明細 `(<主表單數>_id, sort_order, id) WHERE deleted_at IS NULL`；表中列出的外鍵索引。

#### `sales_invoices`（銷項發票，系統單據 `sales_invoice`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| invoice_type | TEXT NOT NULL CHECK | `select` | 發票種類 | 必填、預設 `triplicate`；選項 `triplicate`（三聯式）/ `duplicate`（二聯式） |
| invoice_no | TEXT NOT NULL | `text` | 發票號碼 | 必填、可搜尋；格式見「發票號碼」 |
| invoice_date | DATE NOT NULL | `date` | 發票日期 | 必填。桌面程式新增時預設今天 |
| tax_period | TEXT NOT NULL | `text` | 期別 | 系統維護；見「期別」 |
| customer_id | UUID NOT NULL FK → customers | `relation` | 客戶 | 必填、→ `customer` |
| buyer_tax_id | TEXT | `text` | 買受人統一編號 | 可搜尋；有填時必須是 8 位數字；三聯式開立時必填；見「買受人資訊」 |
| buyer_name | TEXT | `text` | 買受人 | 發票抬頭；三聯式開立時必填 |
| buyer_address | TEXT | `text` | 買受人地址 | |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 必填；選項與商品的 `tax_type` 相同 |
| tax_amount_manual | NUMERIC(18,4) CHECK (tax_amount_manual >= 0) | `money` | 稅額（手動） | 小數 4、`min 0`；留空 = 依系統計算；見「金額與稅額」 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft`（草稿）/ `posted`（已開立）/ `void`（已作廢） |
| subtotal / tax_amount / total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 未稅金額 / 稅額 / 總計 | 系統維護、小數 4 |
| void_date | DATE | `date` | 作廢日期 | 系統維護 |
| void_reason | TEXT | `text` | 作廢原因 | 系統維護 |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(lower(invoice_no))`（可搜尋）與唯一索引 `uq_sales_invoices_invoice_no`（見「發票號碼」）。

#### `sales_invoice_lines`（明細子表 `sales_invoice_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| sales_invoice_id | UUID NOT NULL FK → sales_invoices | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| product_id | UUID FK → products | `relation` | 商品 | → `product`；可以不填（合併的列、運費等） |
| description | TEXT NOT NULL | `text` | 品名 | 必填、`max_length 200`；發票上印的品名 |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 必填、小數 4、`min 0`；三聯式**未稅**、二聯式**含稅** |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4 |
| notes | TEXT | `long_text` | 備註 | |

另外有索引 `(product_id)`。發票明細沒有折扣率，帶入時折算到單價（見「關聯出貨單」）。

#### `sales_invoice_shipments`（明細子表 `sales_invoice_shipment`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| sales_invoice_id | UUID NOT NULL FK → sales_invoices | （不是欄位） | | 所屬發票 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 順序 |
| shipment_id | UUID NOT NULL FK → shipments | `relation` | 出貨單 | 必填、→ `shipment`；見「關聯出貨單」 |

另外有索引 `(shipment_id)`。

#### `sales_allowances`（銷貨折讓，系統單據 `sales_allowance`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1；折讓證明單沒有發票號碼，以單號識別 |
| sales_invoice_id | UUID NOT NULL FK → sales_invoices | `relation` | 原發票 | 必填、→ `sales_invoice`；新增後不可修改（`422`，「建立後不可更換發票」） |
| customer_id | UUID NOT NULL FK → customers | `relation` | 客戶 | 系統維護；= 發票的客戶 |
| allowance_date | DATE NOT NULL | `date` | 折讓日期 | 必填。桌面程式新增時預設今天；不可早於發票日期 |
| tax_period | TEXT NOT NULL | `text` | 期別 | 系統維護；由 `allowance_date` 計算 |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 系統維護；= 發票的稅別 |
| sales_return_id | UUID FK → sales_returns | `relation` | 銷貨退回 | → `sales_return`；見「銷貨折讓」 |
| tax_amount_manual | NUMERIC(18,4) CHECK (tax_amount_manual >= 0) | `money` | 稅額（手動） | 同銷項發票 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft` / `posted`（已開立）/ `void` |
| subtotal / tax_amount / total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 折讓未稅金額 / 折讓稅額 / 折讓總計 | 系統維護、小數 4 |
| reason | TEXT | `text` | 折讓原因 | |
| void_date / void_reason | | | 作廢日期 / 作廢原因 | 系統維護 |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(sales_invoice_id)`、`(sales_return_id)`。

#### `sales_allowance_lines`（明細子表 `sales_allowance_line`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| sales_allowance_id | UUID NOT NULL FK → sales_allowances | （不是欄位） | | 所屬單據 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 明細順序 |
| product_id | UUID FK → products | `relation` | 商品 | → `product`；可以不填 |
| description | TEXT NOT NULL | `text` | 品名 | 必填、`max_length 200` |
| qty | NUMERIC(18,4) NOT NULL CHECK (qty > 0) | `decimal` | 數量 | 必填、小數 4、`min 0`；必須大於 0 |
| unit_price | NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0) | `money` | 單價 | 必填、小數 4、`min 0`；一律**未稅**（折讓證明單分列未稅金額與稅額） |
| amount | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 金額 | 系統維護、小數 4 |
| notes | TEXT | `long_text` | 備註 | |

另外有索引 `(product_id)`。

#### `purchase_invoices`（進項憑證，系統單據 `purchase_invoice`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| doc_type | TEXT NOT NULL CHECK | `select` | 憑證種類 | 必填、預設 `triplicate`；選項 `triplicate`（三聯式發票）/ `electronic`（電子發票）/ `customs`（海關代徵營業稅繳納證）；開立後不可修改（草稿以外本來就不可修改） |
| invoice_no | TEXT NOT NULL | `text` | 憑證號碼 | 必填、可搜尋；格式見「發票號碼」 |
| invoice_date | DATE NOT NULL | `date` | 憑證日期 | 必填 |
| tax_period | TEXT NOT NULL | `text` | 期別 | 系統維護 |
| supplier_id | UUID NOT NULL FK → suppliers | `relation` | 廠商 | 必填、→ `supplier`；海關繳納證填該批進口貨物的國外廠商 |
| seller_tax_id | TEXT | `text` | 賣方統一編號 | 有填時必須是 8 位數字；新增時沒有帶時帶入廠商的 `tax_id`（快照）；海關繳納證可以不填 |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 必填、預設 `taxable` |
| subtotal | NUMERIC(18,4) NOT NULL CHECK (subtotal >= 0) | `money` | 未稅金額 | 必填、小數 4、`min 0`；海關繳納證為完稅價格 |
| tax_amount | NUMERIC(18,4) NOT NULL CHECK (tax_amount >= 0) | `money` | 稅額 | 必填、小數 4、`min 0`；見「金額與稅額」 |
| total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 總計 | 系統維護、小數 4；`subtotal + tax_amount` |
| deductible | BOOLEAN NOT NULL DEFAULT true | `boolean` | 可扣抵 | 必填、預設 `true`；進項稅額是否可扣抵（例如交際費不可扣抵），只是紀錄 |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft` / `posted`（已登錄）/ `void` |
| void_date / void_reason | | | 作廢日期 / 作廢原因 | 系統維護 |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(lower(invoice_no))` 與唯一索引 `uq_purchase_invoices_invoice_no`（見「發票號碼」）。進項憑證的作廢通常代表登錄錯誤或廠商作廢重開。

#### `purchase_invoice_receipts`（明細子表 `purchase_invoice_receipt`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| purchase_invoice_id | UUID NOT NULL FK → purchase_invoices | （不是欄位） | | 所屬憑證 |
| sort_order | INTEGER NOT NULL DEFAULT 0 | （不是欄位） | | 順序 |
| goods_receipt_id | UUID NOT NULL FK → goods_receipts | `relation` | 進貨單 | 必填、→ `goods_receipt`；見「關聯出貨單、進貨單」 |

另外有索引 `(goods_receipt_id)`。

#### `purchase_allowances`（進貨折讓，系統單據 `purchase_allowance`）

| 欄位 | 型態 | data_type | 顯示名稱 | 定義 |
|---|---|---|---|---|
| record_no | TEXT NOT NULL | （單號，不是欄位） | | §7.1 |
| purchase_invoice_id | UUID NOT NULL FK → purchase_invoices | `relation` | 原憑證 | 必填、→ `purchase_invoice`；不可是海關繳納證；新增後不可修改（`422`，「建立後不可更換憑證」） |
| supplier_id | UUID NOT NULL FK → suppliers | `relation` | 廠商 | 系統維護；= 憑證的廠商 |
| allowance_date | DATE NOT NULL | `date` | 折讓日期 | 必填；不可早於憑證日期 |
| tax_period | TEXT NOT NULL | `text` | 期別 | 系統維護 |
| tax_type | TEXT NOT NULL FK → tax_rates(code) | `select` | 稅別 | 系統維護；= 憑證的稅別 |
| purchase_return_id | UUID FK → purchase_returns | `relation` | 進貨退回 | → `purchase_return`；見「進貨折讓」 |
| subtotal | NUMERIC(18,4) NOT NULL CHECK (subtotal >= 0) | `money` | 折讓未稅金額 | 必填、小數 4、`min 0` |
| tax_amount | NUMERIC(18,4) NOT NULL CHECK (tax_amount >= 0) | `money` | 折讓稅額 | 必填、小數 4、`min 0` |
| total | NUMERIC(18,4) NOT NULL DEFAULT 0 | `money` | 折讓總計 | 系統維護；`subtotal + tax_amount` |
| status | TEXT NOT NULL DEFAULT 'draft' CHECK | `select` | 狀態 | 系統維護；選項 `draft` / `posted`（已開立）/ `void` |
| reason | TEXT | `text` | 折讓原因 | |
| void_date / void_reason | | | 作廢日期 / 作廢原因 | 系統維護 |
| notes | TEXT | `long_text` | 備註 | |
| posted_at / posted_by、voided_at / voided_by | | （不是欄位） | | |

另外有索引 `(purchase_invoice_id)`、`(purchase_return_id)`。

#### 發票號碼

存檔時去除前後空白、英文轉大寫後檢查格式（`hammer_core::invoicing::number`）：

| 單據 | 格式 | 錯誤（`invoice_no`） |
|---|---|---|
| 銷項發票；進項憑證的 `triplicate`、`electronic` | `^[A-Z]{2}[0-9]{8}$`（2 碼英文 + 8 碼數字） | 「發票號碼格式應為 2 碼英文加 8 碼數字」 |
| 進項憑證的 `customs` | `^[A-Z0-9]{1,20}$`（英數字，最長 20 碼） | 「繳納證號碼只能是英文與數字，最長 20 碼」 |

**唯一規則**（草稿不檢查重複；**開立時**才檢查，見「開立與作廢」）：

| 單據 | 唯一範圍 | 索引 |
|---|---|---|
| 銷項發票 | 全域唯一，**含已作廢**（作廢的紙本號碼不能再用） | `CREATE UNIQUE INDEX uq_sales_invoices_invoice_no ON sales_invoices (invoice_no) WHERE status <> 'draft'` |
| 進項憑證 | 同一廠商 + 號碼唯一，**不含已作廢**（作廢通常是登錄錯誤，要能重新登錄） | `CREATE UNIQUE INDEX uq_purchase_invoices_invoice_no ON purchase_invoices (supplier_id, invoice_no) WHERE status = 'posted'` |

- 已刪除的草稿 `status` 仍是 `draft`，不在索引內。
- 開立時先以查詢檢查（給出清楚的訊息），並發時由唯一索引擋下；伺服器依索引名稱把違反唯一索引轉成相同的 `422`。
- 重複 → `422 VALIDATION`，`invoice_no`「發票號碼 AB12345678 已使用（SI202611-0003）」；進項為「此廠商的憑證號碼 AB12345678 已登錄（PI202611-0003）」。

#### 期別

營業稅每兩個月一期。`tax_period` 由單據日期（發票、憑證為 `invoice_date`，折讓為 `allowance_date`）計算，每次存檔時重新寫入：

```
tax_period = 年（4 位）+ 該期第一個月（奇數月，2 位）
```

例如 2026-11-19、2026-12-05 → `202611`；2026-02-28 → `202601`。桌面程式顯示為「2026 年 11–12 月」。`tax_period` 可以篩選（`eq`）、排序（文字）。

#### 買受人資訊（銷項發票）

- 新增時 `buyer_tax_id`、`buyer_name`、`buyer_address` **沒有帶到**的，由伺服器從客戶帶入：`buyer_tax_id` = `tax_id`、`buyer_name` = `invoice_title`（空白時用 `name`）、`buyer_address` = `invoice_address`（空白時用 `address`）。修改時 `customer_id` 有變動，且沒有帶到這三個欄位時，重新帶入。
- 帶入後是**快照**：之後修改客戶資料不影響既有發票；草稿可以手動修改。
- **三聯式**開立時 `buyer_tax_id`、`buyer_name` 必填（「三聯式發票必須填寫買受人統一編號」「三聯式發票必須填寫買受人」）；**二聯式**可以不填統編。
- 進項憑證的 `seller_tax_id` 同樣在新增時沒有帶到時帶入廠商的 `tax_id`（快照）。

#### 金額與稅額

**銷項發票、銷貨折讓**與銷貨單據共用 `hammer_core::sales::amounts`（`d` = TWD 的 `decimal_places`、四捨五入 `MidpointAwayFromZero`、稅額以整張計算）。明細沒有折扣率（以 `discount_rate = 0` 呼叫）：

| | 三聯式發票、銷貨折讓 | 二聯式發票 |
|---|---|---|
| 對應 `tax_included` | `false`（單價未稅） | `true`（單價含稅） |
| 明細 `amount` | `round(qty × unit_price, d)` | 同左 |
| 系統計算 | `subtotal = Σ amount`；`tax = round(subtotal × 稅率, d)`；`total = subtotal + tax` | `total = Σ amount`；`subtotal = round(total ÷ (1 + 稅率), d)`；`tax = total − subtotal` |

**手動修正稅額**（`tax_amount_manual`，讓系統金額與紙本一致）：

- 留空 → `tax_amount` = 系統計算的稅額。
- 有值 → 必須是 `d` 位小數以內、`|tax_amount_manual − 系統計算的稅額| ≤ 1`（只用於吸收紙本與系統的四捨五入差異）；`tax_amount = tax_amount_manual`。
  - 三聯式、折讓：`subtotal` 不變，`total = subtotal + tax_amount`（總計跟著變）。
  - 二聯式：紙本上只有總計，`total`（= Σ amount）不變，`subtotal = total − tax_amount`。
- 稅率為 0（`zero_rated`、`exempt`）時不可填（「零稅率、免稅的發票沒有稅額」）。
- 超過 → `422 VALIDATION`，`tax_amount_manual`「稅額與系統計算相差超過 1 元（系統計算 48）」。
- 每次存檔時重新計算並檢查（修改明細後系統稅額改變，原本的手動稅額可能超出範圍而存檔失敗，需要清空或重新輸入）。

**範例一：三聯式、應稅**（`d = 0`、稅率 0.05）

| 明細 | qty | unit_price | amount |
|---|---|---|---|
| 1 | 3 | 100.0000 | 300 |
| 2 | 2 | 333.3333 | round(666.6666, 0) = 667 |

系統計算：`subtotal` = 967；`tax_amount` = round(48.35, 0) = 48；`total` = 1015。
紙本稅額為 49 → `tax_amount_manual` = 49（差 1，允許）→ `tax_amount` = 49、`total` = 1016。填 50 → `422`「稅額與系統計算相差超過 1 元（系統計算 48）」。

**範例二：二聯式、應稅**（`d = 0`、稅率 0.05）

| 明細 | qty | unit_price | amount |
|---|---|---|---|
| 1 | 4 | 200.0000 | 800 |
| 2 | 1 | 200.0000 | 200 |

系統計算：`total` = 1000；`subtotal` = round(952.3809…, 0) = 952；`tax_amount` = 48。
`tax_amount_manual` = 47 → `tax_amount` = 47、`subtotal` = 953、`total` = 1000。

**進項憑證、進貨折讓**沒有明細，`subtotal`、`tax_amount` 由使用者照憑證輸入（`d` 位小數以內），`total = subtotal + tax_amount`：

- `triplicate`、`electronic`、進貨折讓：`|tax_amount − round(subtotal × 稅率, d)| ≤ 1`，超過 → `tax_amount`「稅額與未稅金額 × 稅率相差超過 1 元（應為 48）」；稅率為 0 時 `tax_amount` 必須是 0。
- `customs`：海關依完稅價格（含關稅）計算，不檢查稅額與 `subtotal` 的關係。

#### 關聯出貨單、進貨單

銷項發票以 `lines.sales_invoice_shipment` 關聯 0–多張出貨單，進項憑證以 `lines.purchase_invoice_receipt` 關聯 0–多張進貨單。關聯**不強制**（可以開沒有出貨單的發票），發票明細也不必與出貨明細一致。

**存檔時與開立時都檢查**（錯誤在該列的 `shipment_id` / `goods_receipt_id`）：

| 檢查 | 訊息 |
|---|---|
| 出貨單（進貨單）未刪除、`status = posted` | 「出貨單尚未過帳」「出貨單已作廢」（進貨單同） |
| 客戶（廠商）與發票（憑證）相同 | 「客戶與發票不同」「廠商與憑證不同」 |
| 同一張發票中不重複 | 第二列「出貨單重複」「進貨單重複」 |
| **一張出貨單只能在一張有效發票中**：沒有其他 `status = posted` 的銷項發票關聯它 | 「出貨單 SH202611-0005 已開立發票 AB12345678」 |
| 進貨單：同一**類**憑證只能一張有效——發票類（`triplicate`、`electronic`）與海關繳納證（`customs`）各自計算 | 「進貨單 GR202611-0005 已有憑證 AB12345678」 |

- 草稿不保留關聯（與退回數量相同，§8.8）：兩張草稿可以關聯同一張出貨單，先開立的成功，後開立的在開立時失敗。
- 進口貨物同一張進貨單可以同時有一張發票類憑證與一張海關繳納證。

**帶入明細**是桌面程式的功能（不另外提供伺服器端點）：使用者勾選出貨單後，桌面程式以單筆查詢讀取每張出貨單，依出貨日期、`sort_order` 把出貨明細加到發票明細：

| 發票明細欄位 | 帶入的值 |
|---|---|
| `product_id` | 出貨明細的商品 |
| `description` | 商品的 `name`（有 `spec` 時為 `name spec`） |
| `qty` | 出貨明細的 `qty` |
| `unit_price` | `p = 出貨明細的 unit_price × (1 − discount_rate)`，再依發票種類與出貨單的 `tax_included` 換算：相同基準 → `round(p, 4)`；三聯式但出貨單含稅 → `round(p ÷ (1 + 稅率), 4)`；二聯式但出貨單未稅 → `round(p × (1 + 稅率), 4)` |

- 表頭的 `customer_id`、`tax_type` 帶入第一張出貨單的值（客戶已選時不變）；出貨單的稅別不同時桌面程式顯示提示，不檢查。
- 帶入後可以自由修改、合併、刪除、新增列；取消勾選出貨單時，桌面程式詢問是否一併移除帶入的列（明細不記錄來源，所以由桌面程式在當次編輯中追蹤）。
- 取消勾選不需要伺服器處理：整組取代 `lines.sales_invoice_shipment` 即可。

#### 開立與作廢

使用 §10「單據動作」的 `post`、`void` 端點（需要 `record.post`；檢查順序、回應、`INVALID_STATUS` 相同）。

**作廢的 request**：這四種單據的 `void` 接受 `{ "version": 3, "void_date": "2026-11-20", "void_reason": "金額開錯" }`（`DocumentActionRequest` 的選填欄位 `void_date`、`void_reason`）：

- `void_reason` 必填（去除空白後不可為空，「請輸入作廢原因」，`details.fields` 指出 `void_reason`）；`void_date` 可省略，預設為作廢當天（Asia/Taipei），不可早於單據日期（「作廢日期不可早於發票日期」）。
- 其他單據的 `void`、所有單據的其他動作帶了 `void_date` 或 `void_reason` → `422 VALIDATION`（「此動作不接受作廢日期與原因」）。
- **跨期作廢不擋**：桌面程式在 `void_date` 的期別與單據的 `tax_period` 不同時顯示警告「作廢日期與發票不同期（發票 2026 年 9–10 月），請確認是否需要另外處理申報」，使用者確認後仍可作廢。

**銷項發票開立**（鎖定順序：發票（`FOR UPDATE`）→ 發票號碼（advisory lock）→ 客戶（`FOR SHARE`）→ 關聯的出貨單（依 id `FOR UPDATE`））：

1. 檢查 `version`、`status = draft`；重新執行存檔時的檢查（號碼格式、金額與手動稅額、關聯出貨單）。
2. 三聯式的買受人必填；至少一列明細（「至少要有一筆明細」）；客戶未刪除（「客戶已刪除」）。
3. 鎖定出貨單後再檢查一次「一張出貨單只能在一張有效發票中」（並行開立時，後取得鎖的一方在這裡失敗）。
4. 發票號碼不重複（「發票號碼」）。查詢前以 advisory lock 鎖定號碼，並行開立同一個號碼時後者在查詢時就會看到前者（不會撞上唯一索引）。
5. `status = posted`、`posted_at`、`posted_by`，`version` + 1；寫入操作紀錄（`post`）。

**銷項發票作廢**：

1. 檢查 `version`、`status = posted`；`void_reason`、`void_date`。
2. 沒有**有效的銷貨折讓**（指向此發票、`status = posted`）→ 否則 `422 VALIDATION`，`details` 為 `{ "fields": [], "documents": [ … ] }`，訊息「此發票已有銷貨折讓 SA202611-0002，請先作廢折讓單」。
3. `status = void`、`voided_at`、`voided_by`、`void_date`、`void_reason`，`version` + 1；寫入操作紀錄（`void`）。關聯的出貨單隨即解除鎖定（可以再開立在新發票，也可以作廢）。

**進項憑證**的開立、作廢與銷項發票相同（廠商、進貨單、「此憑證已有進貨折讓 PA202611-0002，請先作廢折讓單」），差別：不檢查買受人；沒有明細，所以不檢查「至少一筆明細」；`subtotal` 為 0 時「未稅金額必須大於 0」（海關繳納證也相同）。

#### 銷貨折讓

由桌面程式從**已開立的發票**建立：表頭帶入發票，伺服器在存檔時帶入 `customer_id`、`tax_type`（系統維護）。

- 存檔與開立時檢查發票 `status = posted`（`sales_invoice_id`「發票尚未開立」「發票已作廢」）；`allowance_date ≥ invoice_date`（「折讓日期不可早於發票日期」）。
- `sales_return_id` 有值時：銷貨退回未刪除、`status = posted`（「銷貨退回尚未過帳」「銷貨退回已作廢」）、客戶與發票相同（「客戶與發票不同」）、**沒有其他有效的銷貨折讓**關聯它（「銷貨退回 SR202611-0002 已有折讓單 SA202611-0001」）。不檢查退回的出貨單是否在這張發票中（退貨可能沖別張發票）。
- 桌面程式選擇銷貨退回時可以把退回明細帶入折讓明細（`description`、`qty`、`unit_price` 換算為未稅，同「帶入明細」），也可以手動輸入（單純降價）。

**累計上限**（存檔時與開立時都檢查；開立時在鎖定發票後檢查）：

```
可折讓金額 = 發票的 total − 該發票其他「已開立」折讓的 total 合計
```

- 超過 → `422 VALIDATION`，`details.fields` 指出 `total`，訊息「超過可折讓金額 1,050（發票總計 2,100，已折讓 1,050）」（金額依 `d` 位顯示，千分位）。
- 作廢折讓後額度恢復；草稿不佔額度。

**開立**（鎖定順序：折讓（`FOR UPDATE`）→ 發票（`FOR UPDATE`，同一張發票的折讓依序開立）→ 銷貨退回（有值時，`FOR UPDATE`））：重新執行存檔時的檢查、至少一列明細、累計上限、銷貨退回只在一張有效折讓中；`status = posted`、操作紀錄。

**作廢**：隨時可以作廢已開立的折讓（`void_reason` 必填，同發票）；作廢後發票的可折讓金額恢復、銷貨退回可以再關聯或作廢。

#### 進貨折讓

與銷貨折讓相同（指向**已登錄**的進項憑證、`supplier_id`、`tax_type` 由憑證帶入、累計上限、鎖定順序、作廢），差別：

- 憑證不可是 `customs`（`purchase_invoice_id`「海關繳納證不可折讓」）。
- 沒有明細，`subtotal`、`tax_amount` 由使用者輸入（「金額與稅額」）；`subtotal` 必須大於 0。
- `purchase_return_id`：進貨退回已過帳、廠商相同、沒有其他有效的進貨折讓關聯它（「進貨退回 PR202611-0002 已有折讓單 PA202611-0001」）。
- 訊息中的「發票」改為「憑證」，例如「超過可折讓金額 1,050（憑證總計 2,100，已折讓 1,050）」。

#### 其他單據的作廢限制

| 單據 | 限制 | 錯誤（`422 VALIDATION`，`details` 為 `{ "fields": [], "documents": [ { "entity_key", "record_id", "record_no" } ] }`） |
|---|---|---|
| 出貨單 | 被有效（`posted`）的銷項發票關聯時不可作廢 | 「此出貨單已開立發票 AB12345678，請先作廢發票」 |
| 進貨單 | 被有效的進項憑證關聯時不可作廢 | 「此進貨單已有進項憑證 AB12345678，請先作廢憑證」 |
| 銷貨退回 | 被有效的銷貨折讓關聯時不可作廢 | 「此銷貨退回已有折讓單 SA202611-0001，請先作廢折讓單」 |
| 進貨退回 | 被有效的進貨折讓關聯時不可作廢 | 「此進貨退回已有折讓單 PA202611-0001，請先作廢折讓單」 |

- `documents` 的 `record_no` 為發票、憑證的系統單號；訊息中發票、憑證顯示 `invoice_no`，折讓顯示 `record_no`。多張時訊息為第一張。
- 檢查位置：在既有的「沒有有效的退回」檢查之後（§8.8、§8.9）。作廢時已先 `FOR UPDATE` 鎖定該單據，而開立發票（折讓）時也以 `FOR UPDATE` 鎖定出貨單（退回單），所以兩邊不會同時成功。

#### 客戶、廠商被引用時的限制

補充 §8.8、§8.9「被單據引用時的限制」：

| 資料 | 規則 |
|---|---|
| 客戶 | 沒有銷貨單據、但有任何未刪除的銷項發票、銷貨折讓（不論狀態）指向它時不可刪除：`details` 為 `{ "fields": [], "invoice_document_count": 2 }`，訊息「此客戶已有 2 張發票或折讓單，無法刪除，請改為停用」。同時有銷貨單據時維持 §8.9 原本的錯誤 |
| 廠商 | 同上（進項憑證、進貨折讓），訊息「此廠商已有 2 張進項憑證或折讓單，無法刪除，請改為停用」 |
| 商品 | 發票、折讓明細的 `product_id` 只是參考，**不限制**刪除與修改（已刪除的商品在關聯欄位顯示 `deleted = true`，§7.3） |

鎖定與 §8.8 相同（刪除時 `FOR UPDATE` 鎖定客戶、廠商再計數；單據存檔時 `FOR SHARE`）。

#### 查詢 API

`hammer_shared::invoice` 定義型別，需要 `record.read`，分頁依 §10。

**尚未開立發票的出貨單** `GET /api/v1/invoicing/uninvoiced-shipments?customer_id=&date_from=&date_to=&page=&page_size=`：

- 列出 `status = posted`、未刪除、**沒有被任何 `status = posted` 的銷項發票關聯**的出貨單；條件都可省略，日期為 `ship_date`，含開始、含結束；依 `ship_date, id` 排序。
- 項目：`{ "id", "record_no", "ship_date", "customer_id", "customer_code", "customer_name", "tax_type", "tax_included", "subtotal", "tax_amount", "total", "returned_total", "net_total" }`；`returned_total` = 指向它的已過帳銷貨退回 `total` 合計，`net_total = total − returned_total`（參考用，發票金額仍由使用者決定）。

**尚未取得憑證的進貨單** `GET /api/v1/invoicing/uninvoiced-receipts?supplier_id=&date_from=&date_to=&doc_class=invoice|customs&page=&page_size=`：

- 列出已過帳、未刪除、沒有被該類（`doc_class` 預設 `invoice` = `triplicate`、`electronic`；`customs` = 海關繳納證）有效進項憑證關聯的進貨單；日期為 `receipt_date`。
- 項目：`{ "id", "record_no", "receipt_date", "supplier_id", "supplier_code", "supplier_name", "currency_code", "exchange_rate", "tax_type", "total", "total_twd", "returned_total", "net_total" }`（退回、淨額為單據幣別）。

**單據的發票** `GET /api/v1/invoicing/links?entity_key=shipment&record_id=…`（`entity_key` 為 `shipment`、`goods_receipt`、`sales_return`、`purchase_return`）：回傳關聯此單據的發票、憑證或折讓（未刪除，**含草稿與已作廢**，不分頁），`[ { "entity_key", "record_id", "record_no", "invoice_no", "invoice_date", "status" } ]`，依 `invoice_date, id` 排序；折讓的 `invoice_no` 為 `null`、`invoice_date` 為折讓日期。出貨單、進貨單等畫面用來顯示「已開立的發票號碼」。單據不存在 → `404`。

`entity_key`、`doc_class` 不是允許的值 → `422 VALIDATION`（`details.field` 為參數名稱）。

### 8.12 財務模組 ⏳ 待討論

先保留以下方向：
- 應收、應付帳款，以及收付款沖帳
- 外幣應付的匯兌損益
- 是否要做完整的複式記帳（會計科目、傳票、總帳）

---

## 9. 核心業務流程

> 過帳、作廢的共通規則（端點、權限、錯誤碼）見 §10「單據動作」；鎖定一律依 §8.10「鎖定順序」；盤點調整單的完整流程見 §8.10。

### 9.1 進貨過帳

完整規則見 §8.8「進貨過帳與作廢」。

```
1. 鎖定進貨單（version 檢查）→ 採購單 → 商品 → 採購單明細
2. 重新驗證（含採購單狀態、未進貨數量、匯率必填）
3. 每一行明細：
   a. unit_cost_twd = round(unit_price × exchange_rate, 4)（未稅單價）
   b. goods：建立 inventory_lots（lot_no 留空時自動產生）、新增 inventory_transactions（receipt，+qty）
      service：不建立批號與異動
   c. 如果有對應的採購單明細：累加 received_qty
4. 重新計算採購單狀態（partially_received / closed）
5. status = posted
6. 寫入 audit_logs
（以上全部在同一個資料庫交易內完成）
```

### 9.2 出貨過帳（先進先出或手動指定批號）

完整規則見 §8.9「出貨的批號」「出貨過帳與作廢」。

```
1. 鎖定出貨單（version 檢查）→ 訂單 → 商品 → 批號（指定的批號 + 先進先出列的商品所有可用批號）→ 訂單明細
2. 重新驗證（含訂單狀態、未出貨數量、批號存在）
3. 扣庫存（goods 才處理；service 跳過）：
   a. 先處理有 lot_no 的列（手動指定批號）：逐一檢查 qty ≤ qty_remaining
   b. 再依 sort_order 處理沒有 lot_no 的列：依 received_date, id 由舊到新扣，直到扣滿數量
   c. 每次扣除：建立 lot_allocations、扣減 qty_remaining、新增 inventory_transactions（shipment，−qty）
   d. 庫存不足 → 整張單據失敗、回復原狀，回傳所有短缺（STOCK_INSUFFICIENT）
4. cost_amount_twd = round(Σ allocation.qty × allocation.unit_cost_twd, 2)；累加訂單的 shipped_qty
5. 重新計算訂單狀態（partially_shipped / closed）；cost_total_twd = Σ cost_amount_twd
6. status = posted；寫入 audit_logs
（以上全部在同一個資料庫交易內完成）
```

出貨單草稿階段，桌面程式以「建議批號 API」（§8.9）顯示先進先出的試算結果，使用者可以改為手動指定批號（拆成每個批號一列）。

### 9.3 作廢規則

| 單據 | 作廢條件 | 作廢動作 |
|---|---|---|
| 出貨單 | 沒有有效的銷貨退回；沒有被有效的銷項發票關聯（§8.11） | 依 lot_allocations 把數量加回原批號，新增反向異動，扣回訂單的 `shipped_qty` 並重新計算狀態（§8.9） |
| 進貨單 | 沒有有效的進貨退回；沒有被有效的進項憑證關聯（§8.11）；產生的批號沒有其他單據的有效異動（§8.10「作廢」；此時 qty_remaining = qty_in） | 刪除批號（軟刪除），新增反向異動，扣回採購單的 `received_qty` 並重新計算狀態；已被使用時拒絕（`LOT_IN_USE`），提示先作廢後續單據（§8.8） |
| 進貨退回 | 沒有被有效的進貨折讓關聯（§8.11） | 依 lot_allocations 把數量加回原批號，新增反向異動（§8.8） |
| 採購單 | 只有 `confirmed`（沒有任何有效的進貨）；有進貨時改為手動結案 | 狀態改為 `void`，不影響庫存（§8.8「採購單的狀態」） |
| 盤點調整單 | 新批號沒有其他單據的有效異動；加回既有批號的數量還在 | 依 §8.10「作廢」反向處理 |
| 銷項發票、進項憑證 | 沒有有效的折讓；跨期作廢不擋（桌面程式警告） | 記錄作廢日期與原因，解除關聯的出貨單、進貨單（§8.11） |
| 銷貨折讓、進貨折讓 | 隨時 | 記錄作廢日期與原因，恢復可折讓金額（§8.11） |
| 訂單 | 只有 `confirmed`（沒有任何有效的出貨）；有出貨時改為手動結案 | 狀態改為 `void`，不影響庫存（§8.9「訂單的狀態」） |
| 銷貨退回 | 沒有被有效的銷貨折讓關聯（§8.11）；加回的數量仍在原批號中（未被再出貨或盤虧） | 扣回加回的數量，新增反向異動；不足時拒絕（`STOCK_INSUFFICIENT`，§8.9） |

### 9.4 樂觀鎖定

- 更新時使用 `UPDATE … SET …, version = version + 1 WHERE id = $1 AND version = $2`。
- 影響筆數為 0 時，回傳 `409 Conflict`。介面提示「此資料已被其他人修改，請重新載入」。

---

## 10. API 設計原則

- REST + JSON，路徑前綴 `/api/v1`。
- 認證：`Authorization: Bearer <session token>`。
- 客戶端在 `X-Client-Version` 標頭帶上版本號。版本低於伺服器要求時回傳 `426`，介面提示更新。
- **通用資料 API**：`/api/v1/entities/{entity_key}/records`（列表、單筆、新增、修改、刪除、`lookup`）。自訂資料類別與系統資料類別（M3 起的 `unit`、`product_category`、`product`、`customer`、`supplier` 與其明細子表）都使用這組端點，行為相同（§7.1「系統資料類別共用通用資料 API」）。
- **專屬端點**，只用於以下兩種情況：
  - 系統資料類別在一般新增、修改、刪除以外的動作：系統單據的過帳、作廢（見下方「單據動作」），這些動作有自己的權限（`record.post`）與規則。單據的一般新增、修改、刪除仍使用通用資料 API。
  - 不是資料類別的資料：幣別 `/api/v1/currencies`、匯率 `/api/v1/exchange-rates`、稅別 `/api/v1/tax-rates`（§8.6）、庫存查詢 `/api/v1/inventory/…`（§8.10；建議批號 §8.9）。它們沒有自訂欄位、版面、關聯，也不出現在 `GET /api/v1/entities`。
- 重新計算公式：`POST /api/v1/entities/{entity_key}/recalculate`（需要 `schema.manage`，無 request body），回應 `{ "processed": 120, "changed": 37, "with_errors": 2 }`（型別 `hammer_shared::schema::RecalculateResult`；規則見 §7.6「重新計算」）。
- 表單版面（§7.7、§8.5 `form_layouts`；型別在 `hammer_shared::form_layout`）：
  | 端點 | 權限 | 說明 |
  |---|---|---|
  | `GET /api/v1/entities/{entity_key}/form-layout` | `record.read` | 回應 `{ "id", "version", "is_default", "config" }`；`config` 為正規化後的版面。沒有設定版面時 `is_default = true`，`id`、`version` 為 `null`，`config` 為自動產生的版面 |
  | `PUT /api/v1/entities/{entity_key}/form-layout` | `schema.manage` | request `{ "version": 3, "config": { … } }`（第一次建立時 `version` 為 `null`）；回應同 `GET`，`version` + 1。config 不合法 → `422 VALIDATION`；版本不符 → `409 CONFLICT` |
  | `DELETE /api/v1/entities/{entity_key}/form-layout?version=N` | `schema.manage` | 還原成預設（軟刪除）；回應同 `GET`（`is_default = true`）。沒有設定版面時 `404` |
  - 資料類別不存在、已停用或已刪除、是明細子表 → `404 NOT_FOUND`。
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
- 業務資料列表的篩選與排序（通用資料 API 與 `view_layouts` 的篩選條件使用同一格式，系統資料類別的系統欄位與自訂欄位都適用；型別在 `hammer_shared::record`）：
  - 排序：`sort=<field_key>` 由小到大，`sort=-<field_key>` 由大到小；省略時依建立順序由新到舊（`id DESC`）。只能使用有效（未停用）的欄位。依欄位型態比較（`integer`、`decimal`、`money` 以數值，`date`、`datetime` 以時間），不會按字串排序；沒有值的資料排在最後。
  - 篩選：`filter.<field_key>.<op>=<value>`，例如 `filter.qty.gte=10`。多個條件須全部符合（AND）。
    | op | 說明 | 適用型態 |
    |---|---|---|
    | `eq` | 等於（`relation` 的值為資料 id） | `multi_select` 以外 |
    | `contains` | 包含（不分大小寫） | `text`、`long_text` |
    | `gt`、`gte`、`lt`、`lte` | 大於、大於等於、小於、小於等於 | `integer`、`decimal`、`money`、`date`、`datetime` |
    | `empty` | `true`：沒有值；`false`：有值 | 全部（`file` 只支援這個） |
    | `any` | 包含任一選項；可重複帶多個（`filter.tags.any=a&filter.tags.any=b`），符合任一個即可 | `multi_select` |
  - 值的格式：數值 `12.5`、日期 `YYYY-MM-DD`、日期時間 RFC 3339、勾選 `true` / `false`。值為空字串的條件視為未指定。
  - `relation` 只支援 `eq`、`empty`，不能排序（§7.3）。
  - `formula` 依它的 `formula_result_type` 套用上表的運算子、值的格式與排序方式（例如結果為 `money` 時以數值比較與排序）。
  - 欄位不存在或已停用、運算子不適用於該型態、值的格式錯誤 → `422 VALIDATION`，`details.field` 為參數名稱（例如 `filter.qty.gte`）。不認得的參數名稱忽略。
  - 單號：`sort` 與 `filter` 可使用 `record_no`（保留的代碼，不是欄位），依文字排序，篩選只支援 `eq`、`contains`（§8.4）。
- 主表與明細（明細子表的規則見 §8.5 `custom_records`；型別在 `hammer_shared::record`）：
  - 新增與修改的 payload 以 `lines` 帶明細，鍵為明細子表代碼：
    ```json
    { "version": 3, "data": { … }, "lines": { "parts": [ { "id": "…", "data": { "qty": 2 } }, { "data": { … } } ] } }
    ```
  - 新增：所有明細與主表一起建立；明細不可帶 `id`。
  - 修改：有帶到的明細子表「整組取代」——有 `id` 的更新（`data` 只合併有帶到的欄位）、沒有 `id` 的新增、目前有但沒列出的軟刪除；沒帶到的明細子表不變。`id` 必須是這筆主表在該明細子表的未刪除明細，且不可重複出現。主表與明細都沒有變動時不寫入、`version` 不變。
  - 主表與明細在同一個 transaction 內存檔，任一筆驗證失敗整筆不寫入（也不消耗單號）。
  - 回應：列表、單筆與明細的每一筆資料都有 `formula_errors: { "<field_key>": "<錯誤原因>" }`，列出計算失敗的公式欄位（沒有錯誤時為空物件；§7.6「存檔時計算」）。
  - 回應：單筆查詢與新增、修改的回應有 `lines`，包含主表所有有效的明細子表（沒有明細時為空陣列），每一列的格式與一般資料相同（`record_no`、`lines` 為 `null`），依順序排列；列表不回傳 `lines`。
  - 錯誤：明細不合法時為 `422 VALIDATION`，`details` 為
    ```json
    { "fields": [ { "field_key", "message" } ],
      "lines": [ { "detail_key": "parts", "index": 1, "field_key": "qty", "message": "必須是整數" } ] }
    ```
    `fields` 為主表的欄位錯誤（可能為空）；`index` 為該列在 request 陣列中的位置（從 0 開始），不屬於某一列（明細子表不存在）時為 `null`；`field_key` 不屬於某個欄位（`id` 不存在或重複）時為 `null`。只有主表錯誤時維持原本的 `{ "fields": [ … ] }`。

- 單據動作（系統單據的過帳、作廢；規則見 §7.1「系統單據」，各單據的處理見 §8.10、§9；型別在 `hammer_shared::document`，邏輯在 `hammer_core::documents` 依資料類別分派）：
  | 動作 | 端點 | 前一個狀態 → 之後 |
  |---|---|---|
  | 過帳 | `POST /api/v1/entities/{entity_key}/records/{id}/post` | `draft` → `posted`（採購單、訂單沒有過帳） |
  | 作廢 | `POST /api/v1/entities/{entity_key}/records/{id}/void` | `posted` → `void`（採購單、訂單：`confirmed` → `void`） |
  | 確認（只有採購單、訂單） | `POST /api/v1/entities/{purchase_order 或 sales_order}/records/{id}/confirm` | `draft` → `confirmed` |
  | 結案（只有採購單、訂單） | `POST /api/v1/entities/{purchase_order 或 sales_order}/records/{id}/close` | `confirmed` / `partially_received`（訂單：`partially_shipped`）→ `closed` |
  - 權限：`record.post`（管理者、可編輯者）。
  - request：`{ "version": 3 }`（`DocumentActionRequest`；樂觀鎖定，§9.4）。不接受其他內容；要修改單據請先用通用資料 API 存檔再過帳。例外：發票模組四種單據的 `void` 另外帶 `void_date`、`void_reason`（§8.11「開立與作廢」）。
  - 回應：`200`，內容與單筆查詢相同（含 `lines`、`document`），`version` 已 + 1。
  - 不支援此動作的資料類別（主檔、自訂資料類別、明細子表、不存在的代碼；採購單、訂單的 `post`，採購單、訂單以外的 `confirm`、`close`）、資料不存在或已刪除 → `404 NOT_FOUND`。
  - 檢查順序：權限（`403`）→ 資料存在（`404`）→ `version`（`409 CONFLICT`）→ 狀態（`422 INVALID_STATUS`）→ 內容與庫存（`422 VALIDATION` / `STOCK_INSUFFICIENT` / `LOT_IN_USE`）。任何錯誤都整張 rollback，單據維持原狀態。
  - 過帳時重新驗證整張單據（存檔後被參照的資料可能已改變），格式與存檔相同（`VALIDATION`，`details` 為 `{ "fields", "lines" }`）。
  - 單據相關的錯誤碼（`hammer_shared::error::ErrorCode`，HTTP 都是 `422`）：

    | code | 用途 | `details` |
    |---|---|---|
    | `INVALID_STATUS` | 單據狀態不允許此操作（修改、刪除非草稿；過帳、確認非草稿；作廢非已過帳（採購單、訂單：非已確認）；結案非已確認或部分進貨、部分出貨） | `{ "status": "posted" }`（目前狀態） |
    | `STOCK_INSUFFICIENT` | 過帳或作廢時批號或商品的數量不足 | `{ "items": [ { "product_id", "product_code", "product_name", "lot_id", "lot_no", "required", "available", "shortage", "lines": [ { "detail_key", "index" } ] } ] }` |
    | `LOT_IN_USE` | 作廢時要刪除的批號已被其他有效單據使用 | `{ "lots": [ { "lot_id", "lot_no", "product_id", "product_code", "documents": [ { "entity_key", "record_id", "record_no" } ] } ] }` |

    - `STOCK_INSUFFICIENT` 列出**所有**不足的項目：指定批號時一個批號一項；先進先出（M4c）時一個商品一項，`lot_id`、`lot_no` 為 `null`。同一個批號（商品）出現在多列明細時合併成一項，`required` 為合計，`lines` 列出各列（`index` 為依 `sort_order` 的位置，從 0 開始）。數量為補齊 4 位小數的字串。訊息為第一項，例如「商品 A001 批號 L-01 庫存不足，尚缺 6」；多項時加上「等 3 項」。
    - `LOT_IN_USE` 的訊息例如「批號 L-01 已被其他單據使用，請先作廢 SH202611-0005」。

---

## 11. 維運

| 項目 | 做法 |
|---|---|
| 開發環境 | `deploy/docker-compose.yml` 啟動 PostgreSQL；API Server 用 `cargo run` 啟動 |
| 資料庫版本 | sqlx migrations。伺服器啟動時自動套用 |
| 初始資料 | migration 內建三種角色與權限、內建資料類別與欄位定義、幣別、稅率。第一次啟動時引導建立管理者帳號 |
| 備份 | 每日 `pg_dump`，保留 30 天；並提供管理者手動備份與還原的功能（⏳ 備份存放位置待定） |
| 附件 | 存在伺服器磁碟的附件目錄（`HAMMER_ATTACHMENTS_DIR`；開發預設 `data/attachments`，Docker 為具名 volume `attachments`，掛載在 `/var/lib/hammer/attachments`，重建容器時保留），與資料庫一起備份。⏳ 正式的存放位置與備份方式待 Q4 |
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
| M3 | 客戶、廠商、商品、單位、幣別、匯率等基本資料；關聯欄位指向系統資料類別（M2b 只能關聯自訂資料類別，§7.3） |
| M4a | 單據共通框架（系統單據、單號、狀態、過帳與作廢端點）、庫存基礎（批號、批號扣除、異動流水帳、庫存查詢）、盤點與調整單、期初庫存 |
| M4b | 採購、進貨（含外幣）、進貨退回 |
| M4c | 訂單、出貨（先進先出或手動指定批號）、銷貨退回 |
| M5 | 發票：銷項發票、銷貨折讓、進項憑證、進貨折讓（內部紀錄，§8.11） |
| M6 | 財務（⏳） |
| M7 | 報表、Excel 匯出、列印、備份與還原介面 |
| M8 | 表單畫面自訂排版：表單版面（`form_layouts`）資料表與 API、表單依版面渲染、表單版面設計器（§7.7） |

---

## 13. 待決事項

| # | 項目 | 何時決定 |
|---|---|---|
| Q1 | 發票模組細節（字軌、作廢、折讓、與出貨的關聯） | ✅ 已決定（2026-11-19）：只做內部紀錄，手開號碼不配字軌；銷項三聯式、二聯式，進項三聯式 / 電子發票、海關繳納證；關聯 0–多張出貨單（進貨單），一張出貨單只在一張有效發票中；稅額可在 ±1 元內手動修正；折讓為另一種單據，累計不超過發票總計（§8.11） |
| Q2 | 財務模組深度（是否做複式記帳、匯兌損益） | M6 開始前 |
| Q3 | 進口成本分攤（運費、關稅、報關費）到批號成本 | ✅ 已決定：第一版**不分攤**，批號成本 = `unit_price × exchange_rate`（§8.8）。之後要做時另加「費用分攤單」調整批號成本，不改變既有資料 |
| Q4 | Tailscale 方案、伺服器主機、備份存放位置 | 開放多人使用前 |
| Q5 | 可編輯者能不能維護匯率 | ✅ 已決定：可以（`record.write`）；幣別維持 `settings.manage`（§6.1） |
| Q6 | 程式碼簽章 | 正式發佈前 |
