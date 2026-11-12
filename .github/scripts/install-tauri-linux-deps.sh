#!/usr/bin/env bash
# 安裝在 Linux 上編譯 Tauri（apps/desktop/src-tauri）所需的系統套件。
# workspace 包含桌面程式，clippy 與 cargo test 都會編譯到它。
# 清單見 https://v2.tauri.app/start/prerequisites/#linux
set -euo pipefail

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential \
  libwebkit2gtk-4.1-dev \
  libxdo-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
