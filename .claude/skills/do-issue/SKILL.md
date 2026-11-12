---
name: do-issue
description: 依照專案的 issue 開發規範，完成一個 GitHub issue：讀 issue、開分支、實作、驗證、commit，然後停下來等使用者確認，使用者說 ok 後才推送並開 PR。使用方式：/do-issue <issue 編號或網址>
argument-hint: <issue 編號>
---

# /do-issue

要處理的 issue：`$ARGUMENTS`

規範以 `docs/issue-workflow.md` 為準，架構規則以 `docs/architecture.md` 為準。
**開始前先完整讀過 `docs/issue-workflow.md`**。本檔只寫執行步驟；本檔和規範文件衝突時，以規範文件為準。

與使用者溝通一律使用繁體中文。

---

## 階段一：實作（自動進行）

### 1. 解析參數

- `$ARGUMENTS` 可能是編號（`12`、`#12`）或 issue 網址，取出編號 N。
- 沒有參數時，執行 `gh issue list --state open --limit 20`，列出來請使用者選一個，然後停止。

### 2. 事前檢查

依序執行，任何一項失敗就停下來說明原因，不要自行修復：

```bash
gh auth status
git status --porcelain        # 必須是空的；有未 commit 的改動就停下來問使用者
git switch main && git pull --ff-only
```

如果 `main` 還沒有任何 commit，停下來請使用者先建立初始 commit。

### 3. 讀 issue

```bash
gh issue view N --json number,title,body,labels,milestone,state,comments
```

確認並整理：

- issue 是 open 狀態
- 驗收條件清單（之後逐條對照）
- `area:*` 標籤、類型標籤、里程碑
- 依賴：issue 內文的「依賴 #x」，對每一個執行 `gh issue view x --json state`

**以下情況要停下來問使用者，不要自己猜：**

- 依賴的 issue 還沒關閉
- 沒有驗收條件，或驗收條件模糊到無法驗證
- 需要用到 architecture.md 中標示「⏳ 待討論」的內容，或第 13 節的待決事項（Q1–Q6）
- issue 的要求與 architecture.md 的決策衝突

### 4. 讀相關架構

讀 issue「參考」欄列出的 architecture.md 章節。再依 `area` 標籤補讀：

| 標籤 | 必讀章節 |
|---|---|
| area:db | §8（資料庫設計）、§11 |
| area:backend | §4、§6、§9、§10 |
| area:desktop | §3.3、§5、D5、D13 |
| area:shared | §4（相依方向） |
| area:infra | §3、§11 |

一律要遵守 `docs/issue-workflow.md` §5 的實作硬規則。

### 5. 開分支

依 `docs/issue-workflow.md` §3 命名，type 的判斷：

- 有 `bug` 標籤 → `fix`
- 有 `documentation` 標籤 → `docs`
- 只動到 CI → `ci`；只動到建置、Docker、依賴 → `chore`
- 其他 → `feat`

```bash
git switch -c <type>/N-<英文簡述>
```

如果分支已經存在，停下來問使用者要接續還是重開。

### 6. 向使用者簡報計畫（不需等待確認）

用幾行說明：要改哪些檔案、做法、驗收條件和實作的對應。然後直接開始實作。

### 7. 實作

- 只做 issue 範圍內的事；發現範圍外的問題，記在 PR 的「備註」，不要順手修
- 商業邏輯（`crates/core`、`crates/formula`）要寫單元測試
- 修改 architecture.md 中的決策或資料表設計時，同步更新該文件

### 8. 驗證

依改動範圍執行 `docs/issue-workflow.md` §6 列出的指令。

- 失敗就修正後重跑，直到通過
- 工具或專案結構還不存在（例如 M0 前沒有 Cargo workspace），記下「未執行」和原因，不能當作通過
- 逐條檢查驗收條件是否都已滿足

### 9. Commit

依 `docs/issue-workflow.md` §4 格式，描述用中文：

```
feat(server): 新增登入 API (#12)
```

可以分成多個 commit，但每個 commit 都要能通過建置。**只在本機 commit，不要推送。**

### 10. 停下來等確認

向使用者回報以下內容，然後**停止，等待使用者回覆**：

1. 分支名稱與 commit 列表（`git log --oneline main..HEAD`）
2. 變更檔案摘要（`git diff --stat main...HEAD`）
3. 驗收條件逐條狀態
4. 驗證指令與實際結果（包含未執行的項目和原因）
5. 準備使用的 PR 標題與內文全文
6. 最後一句：「確認沒問題請回覆 ok，我就推送並開 PR。」

使用者要求修改時，修改 → 重新驗證 → commit → 再回到本步驟。

---

## 階段二：開 PR（使用者回覆 ok 之後才執行）

只有在使用者明確回覆 ok（或同等的同意）之後才執行。

```bash
git push -u origin <分支名稱>
gh pr create --base main --title "<標題>" --body-file <暫存檔> \
  --label "<沿用 issue 的標籤，逗號分隔>" --milestone "<issue 的里程碑>"
```

- PR 內文依 `docs/issue-workflow.md` §7 格式，第一行是 `Closes #N`
- issue 沒有里程碑時，省略 `--milestone`
- 完成後回報 PR 網址

**不要 merge PR**，由使用者自行處理。
