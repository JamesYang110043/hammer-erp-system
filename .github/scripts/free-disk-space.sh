#!/usr/bin/env bash
# 清出 GitHub-hosted runner（ubuntu-latest）的磁碟空間。
#
# rust-test 用 cargo llvm-cov 建置整個 workspace（含 Tauri），target 很大；
# Cargo.lock 變動時，rust-cache 收尾還要下載相依套件並儲存新的 cache，
# 曾因 `No space left on device` 失敗（#41）。
#
# 只刪除專案用不到的預裝工具；不存在的目錄直接略過。
set -euo pipefail

echo "::group::清理前"
df -h /
echo "::endgroup::"

dirs=(
  /usr/share/dotnet        # .NET SDK
  /usr/local/lib/android   # Android SDK
  /opt/ghc                 # Haskell
  /usr/local/.ghcup        # Haskell（ghcup）
  /opt/hostedtoolcache/CodeQL
  /usr/local/share/boost
  /usr/share/swift
)
for dir in "${dirs[@]}"; do
  if [ -e "$dir" ]; then
    sudo rm -rf "$dir"
  fi
done

echo "::group::清理後"
df -h /
echo "::endgroup::"
