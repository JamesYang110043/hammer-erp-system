import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { App as AntApp, ConfigProvider } from 'antd';
import zhTW from 'antd/locale/zh_TW';
import { useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

import { AppearanceContext, loadAppearance, resolveColorMode, saveAppearance } from '../appearance';
import type { Appearance } from '../appearance';
import { DARK_COLORS, SEMANTIC_COLORS, appComponentConfig, getAppTheme } from '../theme';

const DARK_QUERY = '(prefers-color-scheme: dark)';

function systemPrefersDark(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia(DARK_QUERY).matches;
}

/**
 * 全站主題：antd ConfigProvider（繁中語系、淺色／深色主題、元件預設值）＋外觀偏好。
 *
 * - 外觀選擇記在 localStorage；跟隨系統時監聽 `prefers-color-scheme` 即時切換
 * - `<html data-theme>` 與 `color-scheme` 跟著切換（捲軸、原生控制項）
 * - 桌面程式中，視窗標題列也跟著切換（Tauri `setTheme`；跟隨系統時交給作業系統）
 */
export function AppThemeProvider({ children }: { children: ReactNode }) {
  const [appearance, setAppearanceState] = useState<Appearance>(loadAppearance);
  const [systemDark, setSystemDark] = useState(systemPrefersDark);

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') {
      return;
    }
    const query = window.matchMedia(DARK_QUERY);
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);

  const mode = resolveColorMode(appearance, systemDark);

  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = mode;
    root.style.colorScheme = mode;
    document.body.style.background =
      mode === 'dark' ? DARK_COLORS.bgLayout : SEMANTIC_COLORS.bgLayout;
  }, [mode]);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    getCurrentWindow()
      .setTheme(appearance === 'system' ? null : appearance)
      .catch(() => {
        // 視窗主題只影響標題列，失敗時不影響畫面
      });
  }, [appearance]);

  const value = useMemo(
    () => ({
      appearance,
      mode,
      setAppearance: (next: Appearance) => {
        setAppearanceState(next);
        saveAppearance(next);
      },
    }),
    [appearance, mode],
  );

  return (
    <AppearanceContext.Provider value={value}>
      <ConfigProvider locale={zhTW} theme={getAppTheme(mode)} {...appComponentConfig}>
        <AntApp>{children}</AntApp>
      </ConfigProvider>
    </AppearanceContext.Provider>
  );
}
