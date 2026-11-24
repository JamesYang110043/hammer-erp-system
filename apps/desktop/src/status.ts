/**
 * 語意狀態與標籤顏色（docs/architecture.md「視覺設計」）。
 *
 * 狀態標籤、select 選項標籤、狀態列都從這裡取色，不直接使用 antd 的色名，
 * 換主題或深色模式時只需要調整 theme.ts 的色票。
 */
import type { BadgeProps } from 'antd';

import type { DocumentStatus } from './bindings';
import { TAG_PALETTE } from './theme';
import type { TagColorName, TagColors } from './theme';

/** 語意狀態。 */
export type StatusTone = 'neutral' | 'success' | 'warning' | 'danger' | 'info';

/** 語意狀態 → 標籤色票。 */
export const TONE_TAG_COLOR: Record<StatusTone, TagColorName> = {
  neutral: 'gray',
  success: 'green',
  warning: 'gold',
  danger: 'red',
  info: 'blue',
};

/** 語意狀態 → Badge 狀態點（顏色來自 antd token：colorSuccess、colorWarning…）。 */
export const TONE_BADGE_STATUS: Record<StatusTone, NonNullable<BadgeProps['status']>> = {
  neutral: 'default',
  success: 'success',
  warning: 'warning',
  danger: 'error',
  info: 'processing',
};

/**
 * 單據狀態的語意：草稿（中性）、已過帳／已開立／已登錄與已結案（成功）、已確認（資訊）、
 * 部分進貨／部分出貨（警告）、已作廢（危險）。
 */
export const DOCUMENT_STATUS_TONES: Record<DocumentStatus, StatusTone> = {
  draft: 'neutral',
  posted: 'success',
  confirmed: 'info',
  partially_received: 'warning',
  partially_shipped: 'warning',
  closed: 'success',
  void: 'danger',
};

/** 語意狀態的標籤顏色。 */
export function toneColors(tone: StatusTone): TagColors {
  return TAG_PALETTE[TONE_TAG_COLOR[tone]];
}

/** select 選項可選的顏色（欄位設定畫面的顏色選擇器，順序即顯示順序）。 */
export const OPTION_COLOR_NAMES: readonly { value: TagColorName; label: string }[] = [
  { value: 'blue', label: '藍' },
  { value: 'green', label: '綠' },
  { value: 'gold', label: '黃' },
  { value: 'orange', label: '橘' },
  { value: 'red', label: '紅' },
  { value: 'purple', label: '紫' },
  { value: 'cyan', label: '青' },
  { value: 'magenta', label: '洋紅' },
];

/**
 * select 選項 `color`（色名字串）→ 標籤色票。
 *
 * 舊資料可能存有 antd 的其他色名或狀態名（`volcano`、`success`…），對應到最接近的色票；
 * 無法辨識或沒有設定時為 `undefined`（顯示預設的灰色標籤）。
 */
export function optionColors(color: string | null | undefined): TagColors | undefined {
  if (!color) {
    return undefined;
  }
  const name = color.trim().toLowerCase();
  if (name in TAG_PALETTE) {
    return TAG_PALETTE[name as TagColorName];
  }
  const alias = LEGACY_COLOR_ALIASES[name];
  return alias ? TAG_PALETTE[alias] : undefined;
}

const LEGACY_COLOR_ALIASES: Record<string, TagColorName> = {
  default: 'gray',
  volcano: 'orange',
  yellow: 'gold',
  lime: 'green',
  geekblue: 'blue',
  pink: 'magenta',
  success: 'green',
  processing: 'blue',
  warning: 'gold',
  error: 'red',
};
