#!/usr/bin/env bash
# 覆蓋率門檻檢查：crates/core、crates/formula 的行覆蓋率低於門檻時回傳非 0。
#
# 用法：coverage-gate.sh <cargo llvm-cov report --json 的輸出檔> [門檻，預設 80]
#
# 結果以 Markdown 表格輸出到 stdout，CI 會把它寫進 job summary 並留言到 PR。
# 某個 crate 沒有任何可量測的程式行時（例如還只是骨架），視為通過。
set -euo pipefail

json="${1:?用法：coverage-gate.sh <coverage.json> [threshold]}"
threshold="${2:-80}"
crates=(core formula)

failed=0
echo "| crate | 行數 | 已覆蓋 | 行覆蓋率 | 門檻 | 結果 |"
echo "|---|---:|---:|---:|---:|---|"

for crate in "${crates[@]}"; do
  # 只計算 crates/<crate>/ 底下的檔案
  read -r count covered < <(
    jq -r --arg p "/crates/${crate}/" '
      [.data[].files[] | select(.filename | contains($p)) | .summary.lines]
      | "\(map(.count) | add // 0) \(map(.covered) | add // 0)"
    ' "$json"
  )

  if [ "$count" -eq 0 ]; then
    echo "| \`${crate}\` | 0 | 0 | - | ${threshold}% | ✅ 無可量測的程式行 |"
    continue
  fi

  percent=$(awk -v c="$covered" -v n="$count" 'BEGIN { printf "%.2f", c * 100 / n }')
  if awk -v p="$percent" -v t="$threshold" 'BEGIN { exit !(p < t) }'; then
    result="❌ 未達門檻"
    failed=1
  else
    result="✅"
  fi
  echo "| \`${crate}\` | ${count} | ${covered} | ${percent}% | ${threshold}% | ${result} |"
done

exit "$failed"
