---
name: do-multi-issue
description: 依序完成多個 GitHub issue：每個 issue 依 /do-issue 實作、驗證、commit，接著直接推送並開 PR，等 CI 全部通過後 merge，切回 main 拉到最新，再做下一個。使用方式：/do-multi-issue <起始編號>~<結束編號>（也可以列出多個編號）
argument-hint: <起始編號>~<結束編號> 或 <編號> <編號> ...
---

# /do-multi-issue

要處理的 issue：`$ARGUMENTS`

本 skill 是 `/do-issue` 的批次版本。**開始前先完整讀過**：

- `.claude/skills/do-issue/SKILL.md`（單一 issue 的執行步驟）
- `docs/issue-workflow.md`（規範，衝突時以規範為準）

和 `/do-issue` 的差異只有三點：

1. 不在階段一結束時停下來等 ok，直接推送並開 PR
2. 等 CI 通過後**自行 merge**（`docs/issue-workflow.md` §7 對本 skill 的例外）
3. merge 後切回 `main` 拉到最新，再接著做下一個 issue

與使用者溝通一律使用繁體中文。

---

## 步驟 0：建立佇列

### 解析參數

支援以下寫法，取出編號清單：

| 輸入 | 結果 |
|---|---|
| `130~135`、`130-135`、`#130~#135` | 130、131、…、135 |
| `130 132 135`、`130,132,135` | 130、132、135 |
| 混用：`130~132 140` | 130、131、132、140 |

- 依編號由小到大排序、去除重複
- 沒有參數時，執行 `gh issue list --state open --limit 20` 列出，請使用者輸入範圍，然後停止
- 範圍超過 10 個編號時，先問使用者確認

### 篩選

對每個編號執行 `gh issue view N --json number,title,state,labels,milestone,url`：

- 不存在，或其實是 PR（`url` 含 `/pull/`）→ 略過
- 已關閉 → 略過
- 其餘排入佇列

### 確認佇列（唯一一次等待）

列出佇列（編號、標題、`area:*` 標籤）與略過的編號和原因，並提醒：

> 每個 issue 會在 CI 通過後直接 merge 進 main，不會停下來讓你 review。

**停下來等使用者回覆 ok**，之後整批自動執行，不再等待。

---

## 每個 issue 的流程

對佇列中的每個 issue 依序執行 A → D。開始時先回報一行：「開始處理 #N（第 i / 共 n 個）：<標題>」。

### A. 實作（同 /do-issue 階段一步驟 2–9）

依 `.claude/skills/do-issue/SKILL.md` 的步驟 2–9 執行：事前檢查、讀 issue、讀架構、開分支、簡報計畫、實作、驗證（含畫面驗證）、commit。

- 步驟 2 的 `git switch main && git pull --ff-only` 每個 issue 都要重做，確保分支建立在上一個 merge 之後的 main
- 依賴的 issue 若在本批前面已 merge，此時應已關閉；仍未關閉就照 /do-issue 的規則停止
- 有畫面驗證時，檢查完截圖後把 `apps/desktop/test-results/` 複製到 scratchpad 的 `issue-N/` 資料夾（下一個 issue 的 E2E 會覆寫這個目錄）
- **跳過 /do-issue 的步驟 10**（不停下來等 ok）

### B. 推送並開 PR（同 /do-issue 階段二）

```bash
git push -u origin <分支名稱>
gh pr create --base main --title "<標題>" --body-file <暫存檔> \
  --label "<沿用 issue 的標籤>" --milestone "<issue 的里程碑>"
```

PR 內文格式同 `docs/issue-workflow.md` §7。在「備註」最後加一行：

> 本 PR 由 `/do-multi-issue` 在 CI 通過後自動 merge。

### C. 等 CI 通過

repo 沒有 branch protection（GitHub Free 方案），GitHub 不會擋下 CI 失敗的 merge，**必須自己確認**。

CI 可能超過 10 分鐘，用 Bash 的 `run_in_background` 執行下面的指令，完成時會收到通知，期間不要輪詢：

```bash
# 等 checks 出現（PR 剛建立時 GitHub 還沒登記 checks），最多 3 分鐘
for i in $(seq 1 18); do
  gh pr checks <PR 編號> >/dev/null 2>&1; rc=$?
  [ $rc -ne 1 ] && break     # 1 = 還沒有 checks；0 = 全部完成；8 = 執行中
  sleep 10
done
gh pr checks <PR 編號> --watch --interval 30
```

完成後用 `gh pr checks <PR 編號> --json name,state,bucket` 判讀：

- 每一項的 `bucket` 都是 `pass` 或 `skipping`（`changes` 判斷不需要的 job 會被跳過，視為通過）→ 進入 D
- 有 `fail` 或 `cancel`：
  1. `gh run view <run id> --log-failed` 看失敗原因
  2. 原因明確出在本 PR 的改動 → 修正、本機重跑對應驗證、commit、push，回到 C 重新等待
  3. 最多修正 2 輪；仍失敗，或原因與本 PR 無關（runner 異常、外部服務、既有的不穩定測試）→ **停止整批**（見「停止條件」）
- 3 分鐘內一直沒有出現 checks → 停止整批

### D. Merge 並回到 main

```bash
gh pr merge <PR 編號> --merge --delete-branch
git switch main
git pull --ff-only
gh issue view N --json state      # 確認已因 Closes #N 自動關閉
```

- 使用 merge commit（與目前 main 的歷史一致）
- issue 沒有自動關閉 → 回報，但不停止
- 回報一行：「#N 已 merge（PR #P），繼續下一個」，然後回到 A 處理下一個 issue

---

## 停止條件

遇到以下任一情況，**停止整批**，不要跳過去做下一個（後面的 issue 可能依賴這一個）：

- /do-issue 規定要停下來問使用者的情況（依賴未關閉、驗收條件模糊、碰到「⏳ 待討論」或 Q1–Q6、與 architecture.md 衝突、分支已存在）
- 本機驗證修不好
- CI 依上面的規則判定失敗
- `gh pr merge` 失敗（例如衝突）
- `git pull --ff-only` 失敗

停止時：

- 保留分支和 PR 的現狀，不要關閉 PR、不要刪分支、不要 force push
- 回報停在哪個 issue、原因、已經做到哪一步，以及佇列中還沒做的編號
- 使用者處理完後，可以用剩下的編號重新執行 `/do-multi-issue`

---

## 全部完成後的總結

整批結束（全部完成或中途停止）時，回報：

1. 結果表：

   | issue | 標題 | PR | 結果 |
   |---|---|---|---|
   | #130 | … | #160 | 已 merge |
   | #131 | … | #161 | 停止：CI rust-test 失敗 |
   | #132 | … | — | 未執行 |

2. 略過的編號與原因
3. **需要人工確認**的項目彙整（各 PR 畫面驗證中 mock 無法涵蓋的部分），標明 PR 編號，提醒以 `pnpm tauri dev` 確認——這些已經 merge 進 main
4. 各 PR 的截圖保存路徑（完整絕對路徑，若有畫面驗證）
