import type { ConfigProviderProps, ThemeConfig } from 'antd';
import { createElement } from 'react';

import { EmptyState } from './components/EmptyState';

/**
 * 全站 antd 主題（main.tsx 與測試的 render.tsx 共用），定案見 docs/architecture.md「視覺設計」。
 *
 * `controlHeight` 同時決定按鈕、輸入框、下拉選單、日期選擇器的高度，調整這一個值就能讓
 * 並排的按鈕與輸入框保持對齊；small / large 尺寸依比例跟著變（27 / 45）。
 * 左側選單的項目高度原本由 `controlHeightLG` 推算，固定為原本的 40，不跟著變。
 */
export const CONTROL_HEIGHT = 36;

/**
 * 品牌色（深藍）與自訂區域（側邊欄、標題列等）用的顏色。
 * 元件不寫死色碼：antd 元件用 token（`theme.useToken()`），antd 沒有對應 token 的地方從這裡取。
 */
export const BRAND_COLORS = {
  /** 主色；antd 依此推導整組色階（hover、active、淺底…） */
  primary: '#1F4E8C',
  /** 側邊欄底色 */
  siderBg: '#12233D',
  /** 側邊欄內展開的子選單底色（比 siderBg 再深一階） */
  siderSubBg: '#0C1A2F',
  /** 側邊欄上的主要文字（標題、選中項目以外的 hover 文字） */
  siderText: '#E6ECF5',
  /** 側邊欄選單項目文字 */
  siderItemText: '#B1BAC7',
  /** 側邊欄區塊標題等次要文字 */
  siderMutedText: '#8792A2',
  /** 側邊欄內的分隔線（Logo 下方、區塊之間） */
  siderDivider: 'rgba(255, 255, 255, 0.08)',
} as const;

/** 語意色與中性色（不隨主色改變）。 */
export const SEMANTIC_COLORS = {
  success: '#2E9D5B',
  warning: '#D98A0B',
  error: '#D64545',
  /** 內容區底色（卡片為白色，靠底色做出層次） */
  bgLayout: '#F3F5F8',
  /** 表格表頭、預設標籤底色 */
  tableHeaderBg: '#F5F7FA',
  /** 表格列 hover 底色（帶一點主色調，與表頭區隔） */
  tableRowHoverBg: '#F0F4FA',
} as const;

/** 中文介面的字型順序；數字另以 `tabular-nums` 對齊（見 styles.css）。 */
export const FONT_FAMILY =
  '-apple-system, BlinkMacSystemFont, "PingFang TC", "Microsoft JhengHei", "Noto Sans TC", "Segoe UI", sans-serif';

export const appTheme: ThemeConfig = {
  token: {
    controlHeight: CONTROL_HEIGHT,
    colorPrimary: BRAND_COLORS.primary,
    colorInfo: BRAND_COLORS.primary,
    colorLink: BRAND_COLORS.primary,
    colorSuccess: SEMANTIC_COLORS.success,
    colorWarning: SEMANTIC_COLORS.warning,
    colorError: SEMANTIC_COLORS.error,
    colorBgLayout: SEMANTIC_COLORS.bgLayout,
    fontFamily: FONT_FAMILY,
    borderRadius: 6,
    borderRadiusLG: 8,
    boxShadowTertiary: '0 1px 2px rgba(16, 24, 40, 0.06), 0 1px 3px rgba(16, 24, 40, 0.08)',
  },
  components: {
    Layout: {
      siderBg: BRAND_COLORS.siderBg,
      headerBg: '#FFFFFF',
      bodyBg: SEMANTIC_COLORS.bgLayout,
      footerBg: '#FFFFFF',
    },
    Menu: {
      itemHeight: 40,
      darkItemBg: BRAND_COLORS.siderBg,
      darkSubMenuItemBg: BRAND_COLORS.siderSubBg,
      darkItemColor: BRAND_COLORS.siderItemText,
      darkItemHoverColor: BRAND_COLORS.siderText,
      darkItemSelectedBg: BRAND_COLORS.primary,
      darkItemSelectedColor: '#FFFFFF',
      darkGroupTitleColor: BRAND_COLORS.siderMutedText,
    },
    Button: { primaryShadow: 'none', defaultShadow: 'none', dangerShadow: 'none' },
    Card: { headerFontSize: 16 },
    Table: {
      headerBg: SEMANTIC_COLORS.tableHeaderBg,
      headerSplitColor: 'transparent',
      rowHoverBg: SEMANTIC_COLORS.tableRowHoverBg,
      // 選取列沿用主色推導的淺底（colorPrimaryBg），不另外指定
    },
    Tag: { defaultBg: SEMANTIC_COLORS.tableHeaderBg },
  },
};

/**
 * 與主題一起套用的元件預設值（main.tsx 與測試的 render.tsx 共用）。
 *
 * - 卡片不加框線、改用淡陰影（`boxShadowTertiary`），在內容區的淺灰底上做出層次
 * - 沒有資料時的畫面統一用 `EmptyState`
 */
export const appComponentConfig = {
  card: { variant: 'borderless' },
  // 表格、清單、下拉選單沒有資料時：一致的空狀態（components/EmptyState.tsx）
  renderEmpty: () => createElement(EmptyState, { compact: true }),
} satisfies Pick<ConfigProviderProps, 'card' | 'renderEmpty'>;
