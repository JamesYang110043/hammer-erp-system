import { theme as antdTheme } from 'antd';
import type { ConfigProviderProps, ThemeConfig } from 'antd';
import { createElement } from 'react';

import { EmptyState } from './components/EmptyState';

/**
 * 全站 antd 主題（AppThemeProvider 套用，main.tsx 與測試的 render.tsx 共用），定案見
 * docs/architecture.md「視覺設計」。淺色與深色各一組（`getAppTheme`）。
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

/** 外觀模式（實際套用的顏色；「跟隨系統」解析後為其中之一，見 src/appearance.ts）。 */
export type ColorMode = 'light' | 'dark';

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
  /** 卡片、標題列、狀態列底色 */
  bgContainer: '#FFFFFF',
  /** 浮層（下拉選單、Modal）底色 */
  bgElevated: '#FFFFFF',
} as const;

/**
 * 深色模式的顏色（#234）：以 antd `darkAlgorithm` 推導其餘色階。
 *
 * 主色在深色底上太暗，按鈕底色改用 `primary`（白字對比 ≥ 4.5:1），連結、資訊文字用較亮的 `link`。
 */
export const DARK_COLORS = {
  primary: '#2F62A6',
  link: '#8AB4E8',
  success: '#3FB872',
  warning: '#E5A23A',
  error: '#EF6B6B',
  bgLayout: '#0F141B',
  bgContainer: '#161C24',
  bgElevated: '#1E252F',
  tableHeaderBg: '#1C232D',
  tableRowHoverBg: '#1F2A38',
  text: '#E3E8EF',
  /** 次要文字（說明、麵包屑、表單提示）；antd 預設的 45% 透明度在深色底上對比不足 */
  textDescription: '#9CA6B4',
} as const;

/** 標籤的一組顏色：文字、底色、框線（文字與底色對比 ≥ 4.5:1，由 theme.test.ts 檢查）。 */
export interface TagColors {
  text: string;
  bg: string;
  border: string;
}

/**
 * 標籤色票（狀態標籤與 select 選項標籤共用，見 src/status.ts）。
 *
 * 色名與資料庫 `options[].color` 相同（維持色名字串）；antd 的預設標籤色在淺底上文字對比不足，
 * 改用加深的文字色。
 */
export const TAG_PALETTE = {
  gray: { text: '#475467', bg: '#F2F4F7', border: '#D0D5DD' },
  blue: { text: '#1F4E8C', bg: '#EAF1FB', border: '#C3D5EE' },
  green: { text: '#1E7A45', bg: '#EAF6EF', border: '#BFE3CC' },
  gold: { text: '#8A5A00', bg: '#FDF5E3', border: '#F2D99B' },
  orange: { text: '#A8470B', bg: '#FDF0E6', border: '#F6CBA6' },
  red: { text: '#B42318', bg: '#FEF0EF', border: '#F7C4BF' },
  purple: { text: '#5B3FA8', bg: '#F2EEFB', border: '#D5CAF0' },
  cyan: { text: '#0E6B78', bg: '#E7F6F8', border: '#B5E1E7' },
  magenta: { text: '#A3226A', bg: '#FBEEF5', border: '#EEC0D8' },
} as const satisfies Record<string, TagColors>;

export type TagColorName = keyof typeof TAG_PALETTE;

/** 深色模式的標籤色票（同一組色名，淺色文字、深色底）。 */
export const DARK_TAG_PALETTE = {
  gray: { text: '#C3CAD5', bg: '#252C36', border: '#3A4350' },
  blue: { text: '#9CC2F0', bg: '#16263D', border: '#284A75' },
  green: { text: '#86D6A6', bg: '#13291D', border: '#25573A' },
  gold: { text: '#F0C76A', bg: '#2E2410', border: '#5E4818' },
  orange: { text: '#F5A874', bg: '#2F1D10', border: '#66391B' },
  red: { text: '#F59C94', bg: '#321716', border: '#6B2C28' },
  purple: { text: '#C2B2F2', bg: '#231C38', border: '#46386E' },
  cyan: { text: '#7FD3DE', bg: '#102A2E', border: '#1F5860' },
  magenta: { text: '#F0A0CB', bg: '#30172A', border: '#662C52' },
} as const satisfies Record<TagColorName, TagColors>;

/** 依外觀模式取得標籤色票。 */
export function tagPalette(mode: ColorMode): Record<TagColorName, TagColors> {
  return mode === 'dark' ? DARK_TAG_PALETTE : TAG_PALETTE;
}

/** 中文介面的字型順序；數字另以 `tabular-nums` 對齊（見 styles.css）。 */
export const FONT_FAMILY =
  '-apple-system, BlinkMacSystemFont, "PingFang TC", "Microsoft JhengHei", "Noto Sans TC", "Segoe UI", sans-serif';

const LIGHT_THEME: ThemeConfig = {
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
      headerBg: SEMANTIC_COLORS.bgContainer,
      bodyBg: SEMANTIC_COLORS.bgLayout,
      footerBg: SEMANTIC_COLORS.bgContainer,
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
    // info 提示：colorInfo 為深藍主色，antd 推導的淺底偏灰，改用標籤色票的藍色底與框線
    Alert: { colorInfoBg: TAG_PALETTE.blue.bg, colorInfoBorder: TAG_PALETTE.blue.border },
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

/** 深色主題：沿用淺色的設定，換掉底色、語意色與主色，其餘色階由 `darkAlgorithm` 推導。 */
const DARK_THEME: ThemeConfig = {
  algorithm: antdTheme.darkAlgorithm,
  token: {
    ...LIGHT_THEME.token,
    colorPrimary: DARK_COLORS.primary,
    colorInfo: DARK_COLORS.primary,
    colorLink: DARK_COLORS.link,
    colorSuccess: DARK_COLORS.success,
    colorWarning: DARK_COLORS.warning,
    colorError: DARK_COLORS.error,
    colorText: DARK_COLORS.text,
    colorTextDescription: DARK_COLORS.textDescription,
    colorBgLayout: DARK_COLORS.bgLayout,
    colorBgContainer: DARK_COLORS.bgContainer,
    colorBgElevated: DARK_COLORS.bgElevated,
    boxShadowTertiary: '0 1px 2px rgba(0, 0, 0, 0.4), 0 1px 3px rgba(0, 0, 0, 0.3)',
  },
  components: {
    ...LIGHT_THEME.components,
    Layout: {
      siderBg: BRAND_COLORS.siderBg,
      headerBg: DARK_COLORS.bgContainer,
      bodyBg: DARK_COLORS.bgLayout,
      footerBg: DARK_COLORS.bgContainer,
    },
    Menu: { ...LIGHT_THEME.components?.Menu, darkItemSelectedBg: DARK_COLORS.primary },
    Alert: {
      colorInfoBg: DARK_TAG_PALETTE.blue.bg,
      colorInfoBorder: DARK_TAG_PALETTE.blue.border,
    },
    Table: {
      headerBg: DARK_COLORS.tableHeaderBg,
      headerSplitColor: 'transparent',
      rowHoverBg: DARK_COLORS.tableRowHoverBg,
    },
    Tag: { defaultBg: DARK_COLORS.tableHeaderBg },
  },
};

/** 依外觀模式取得 antd 主題（AppThemeProvider 使用）。 */
export function getAppTheme(mode: ColorMode): ThemeConfig {
  return mode === 'dark' ? DARK_THEME : LIGHT_THEME;
}

/** 淺色主題（測試與既有程式碼使用）。 */
export const appTheme: ThemeConfig = LIGHT_THEME;

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
