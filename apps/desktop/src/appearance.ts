/**
 * 外觀偏好（#234）：跟隨系統／淺色／深色，存在本機（localStorage）。
 *
 * 伺服器端不儲存；之後有個人設定時再搬。實際套用見 components/AppThemeProvider.tsx。
 */
import { createContext, useContext } from 'react';

import type { ColorMode } from './theme';

export type Appearance = 'system' | 'light' | 'dark';

export const APPEARANCE_STORAGE_KEY = 'hammer.appearance';

export const APPEARANCE_LABELS: Record<Appearance, string> = {
  system: '跟隨系統',
  light: '淺色',
  dark: '深色',
};

export const APPEARANCES: readonly Appearance[] = ['system', 'light', 'dark'];

/** 上次選擇的外觀；沒有存過、格式錯誤或無法存取 localStorage 時為跟隨系統。 */
export function loadAppearance(): Appearance {
  try {
    const raw = window.localStorage.getItem(APPEARANCE_STORAGE_KEY);
    return raw === 'light' || raw === 'dark' || raw === 'system' ? raw : 'system';
  } catch {
    return 'system';
  }
}

/** 儲存外觀；無法寫入時忽略。 */
export function saveAppearance(appearance: Appearance) {
  try {
    window.localStorage.setItem(APPEARANCE_STORAGE_KEY, appearance);
  } catch {
    // 忽略
  }
}

/** 外觀偏好 → 實際套用的模式（跟隨系統時依 `prefers-color-scheme`）。 */
export function resolveColorMode(appearance: Appearance, systemDark: boolean): ColorMode {
  if (appearance === 'system') {
    return systemDark ? 'dark' : 'light';
  }
  return appearance;
}

export interface AppearanceState {
  appearance: Appearance;
  mode: ColorMode;
  setAppearance: (appearance: Appearance) => void;
}

export const AppearanceContext = createContext<AppearanceState>({
  appearance: 'light',
  mode: 'light',
  setAppearance: () => {},
});

/** 目前的外觀偏好與實際模式。 */
export function useAppearance(): AppearanceState {
  return useContext(AppearanceContext);
}
