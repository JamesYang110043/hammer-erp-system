# Issue 開發規範

> 狀態：草案 v0.1（2026-11-12）
> 本文件規定「從 issue 到 PR」的做法，人工開發與 `/do-issue` skill 都以本文件為準。
> 架構與資料庫規則以 [architecture.md](architecture.md) 為準，本文件不重複。

---

## 1. Issue 格式

所有開發 issue 使用 `.github/ISSUE_TEMPLATE/task.md` 模板，必須包含：

| 欄位 | 說明 |
|---|---|
| 背景 | 為什麼要做 |
| 範圍 | 要做什麼；**不做什麼**也要寫 |
| 驗收條件 | 可勾選的清單，每一條都要能被驗證 |
| 依賴 | `依賴 #12`；沒有則寫「無」 |
| 參考 | 對應的 architecture.md 章節 |

標籤與里程碑：

- 至少一個 `area:*` 標籤：`area:backend`、`area:db`、`area:desktop`、`area:shared`、`area:infra`
- 類型標籤：`enhancement`（新功能）、`bug`、`documentation`
- 里程碑：M0–M7（對應 architecture.md 第 12 節）

一個 issue 應該能在一個 PR 內完成。太大就拆。

---

## 2. 開工前檢查

1. 依賴的 issue 都已關閉。
2. 驗收條件明確。不明確 → 先在 issue 留言釐清，不要自己假設。
3. 不碰「⏳ 待討論」或 architecture.md 第 13 節的待決事項（Q1–Q6）。如果碰到 → 停下來，先把決策補進 architecture.md。
4. 從最新的 `main` 開分支。

---

## 3. 分支命名

```
<type>/<issue 編號>-<英文簡述>
```

範例：`feat/12-login-api`、`fix/34-fifo-lot-order`、`chore/3-docker-compose`

- 英文簡述：小寫、用 `-` 連接、3–5 個字
- `type` 與 commit 的 type 相同（見第 4 節）

---

## 4. Commit 格式

採用 Conventional Commits，描述用**中文**：

```
<type>(<scope>): <中文描述> (#<issue 編號>)

<選填：中文說明，為什麼這樣改>
```

範例：`feat(server): 新增登入 API (#12)`

**type**

| type | 用途 |
|---|---|
| feat | 新功能 |
| fix | 修正錯誤 |
| refactor | 重構，不改變行為 |
| test | 只新增或修改測試 |
| docs | 文件 |
| chore | 建置、依賴、雜項 |
| ci | CI 設定 |

**scope**（對應專案結構）

`shared`、`formula`、`core`、`db`、`server`、`desktop`、`deploy`、`ci`、`docs`。跨多個 scope 時，選主要的那個。

規則：

- 每個 commit 都要能通過建置和測試
- 一個 commit 做一件事；migration 和使用它的程式碼可以放在同一個 commit
- 不要 commit 密碼、`.env`、本機設定

---

## 5. 實作硬規則（摘自 architecture.md）

違反以下任一條，PR 不會被接受：

- 模組相依方向不可反向（architecture.md §4）
- 金額一律使用 `rust_decimal` / `NUMERIC(18,4)`，禁止浮點數（D9）
- 主鍵使用 uuid v7（§5）
- 可編輯的資料表有 `version` 欄位，更新時檢查版本（D12、§9.4）
- 權限在伺服器端檢查，不能只靠前端隱藏按鈕（D1、§6）
- 資料表變更一律透過 sqlx migration；修改查詢後更新 `.sqlx/`（§5、§11）
- API 路徑前綴 `/api/v1`，錯誤使用統一格式（§10）
- 寫入資料的操作要留操作紀錄（`audit_logs`）
- 介面文字使用繁體中文；時區 Asia/Taipei（D13）

---

## 6. 完成定義（DoD）

PR 送出前，以下都要成立：

- [ ] 所有驗收條件都已完成
- [ ] 商業邏輯（`crates/core`、`crates/formula`）有對應的單元測試
- [ ] 以下檢查都通過（依改動範圍）：

| 改動範圍 | 指令 |
|---|---|
| Rust | `cargo fmt --all -- --check` |
| Rust | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust | `cargo test --workspace` |
| 資料庫查詢 | `cargo sqlx prepare --workspace --check` |
| 前端 | `npm run lint`、`npx tsc --noEmit`（在 `apps/desktop`） |

- [ ] 如果改動了架構決策或資料表設計，同步更新 architecture.md

尚未建立的工具（例如 M0 之前還沒有 workspace），在 PR 裡註明「未執行」與原因，不要當作通過。

---

## 7. Pull Request

**標題**：與主要 commit 相同格式，例如 `feat(server): 新增登入 API (#12)`

**內文**（使用 `.github/pull_request_template.md`）：

```markdown
Closes #12

## 變更摘要
- …

## 驗收條件
- [x] …（從 issue 複製，逐條勾選）

## 驗證方式
- `cargo test --workspace`：通過
- …（實際執行的指令與結果；未執行的要註明原因）

## 備註
（需要 review 特別注意的地方、後續待辦；沒有寫「無」）
```

其他規則：

- 沿用 issue 的標籤與里程碑
- 一個 PR 只處理一個 issue
- 由專案負責人 review 與 merge；開發者（含 Claude）不自行 merge
