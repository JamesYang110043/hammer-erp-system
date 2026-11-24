import type { ThemeConfig } from 'antd';

/**
 * 全站 antd 主題（main.tsx 與測試的 render.tsx 共用）。
 *
 * `controlHeight` 同時決定按鈕、輸入框、下拉選單、日期選擇器的高度，調整這一個值就能讓
 * 並排的按鈕與輸入框保持對齊；small / large 尺寸依比例跟著變（27 / 45）。
 * 左側選單的項目高度原本由 `controlHeightLG` 推算，固定為原本的 40，不跟著變。
 */
export const CONTROL_HEIGHT = 36;

/** 品牌色組：主色與側邊欄等自訂區域的顏色（元件不寫死色碼，一律從這裡取）。 */
export interface BrandPalette {
  /** 主色；antd 依此推導整組色階（hover、active、淺底…） */
  primary: string;
  /** 側邊欄底色 */
  siderBg: string;
  /** 側邊欄內展開的子選單底色（比 siderBg 再深一階） */
  siderSubBg: string;
  /** 側邊欄上的文字（標題、選單項目） */
  siderText: string;
}

/**
 * 主色方案比較用的預設（#229）。選定後只保留定案的一套，並移除切換器。
 */
export const THEME_PRESETS = {
  /** 深藍：沉穩、偏財務系統 */
  navy: { primary: '#1F4E8C', siderBg: '#12233D', siderSubBg: '#0C1A2F', siderText: '#E6ECF5' },
  /** 藍綠：現代 SaaS 風格 */
  teal: { primary: '#0F8B8D', siderBg: '#0F2B2E', siderSubBg: '#0A2023', siderText: '#E3F1F1' },
} as const satisfies Record<string, BrandPalette>;

export type ThemePresetKey = keyof typeof THEME_PRESETS;

export const DEFAULT_PRESET: ThemePresetKey = 'navy';

/** 中文介面的字型順序；數字另以 `tabular-nums` 對齊（見 styles.css）。 */
export const FONT_FAMILY =
  '-apple-system, BlinkMacSystemFont, "PingFang TC", "Microsoft JhengHei", "Noto Sans TC", "Segoe UI", sans-serif';

/** 語意色與中性色（不隨主色改變）。 */
export const SEMANTIC_COLORS = {
  success: '#2E9D5B',
  warning: '#D98A0B',
  error: '#D64545',
  /** 內容區底色（卡片為白色，靠底色做出層次） */
  bgLayout: '#F3F5F8',
  /** 表格表頭底色 */
  tableHeaderBg: '#F5F7FA',
} as const;

const HEX = /^#?([0-9a-f]{6})$/i;

/** `#1F4E8C` 或 `1F4E8C` → `#1F4E8C`；不是 6 碼色碼時回傳 null。 */
export function normalizeHex(value: string): string | null {
  const m = HEX.exec(value.trim());
  return m ? `#${m[1]!.toUpperCase()}` : null;
}

/** 以 `ratio`（0–1）把 `hex` 混向 `target`。 */
export function mixHex(hex: string, target: string, ratio: number): string {
  const parse = (h: string) => {
    const n = Number.parseInt(h.slice(1), 16);
    return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
  };
  const [a, b] = [parse(hex), parse(target)];
  const out = a.map((c, i) => Math.round(c + (b[i]! - c) * ratio));
  return `#${out.map((c) => c.toString(16).padStart(2, '0')).join('')}`.toUpperCase();
}

/** 自訂品牌色：側邊欄由主色混入黑色推導，文字混入白色。 */
export function customPalette(primary: string): BrandPalette {
  return {
    primary,
    siderBg: mixHex(primary, '#000000', 0.72),
    siderSubBg: mixHex(primary, '#000000', 0.8),
    siderText: mixHex(primary, '#FFFFFF', 0.9),
  };
}

/** 依品牌色組產生 antd 主題。 */
export function buildTheme(palette: BrandPalette): ThemeConfig {
  return {
    token: {
      controlHeight: CONTROL_HEIGHT,
      colorPrimary: palette.primary,
      colorInfo: palette.primary,
      colorLink: palette.primary,
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
        siderBg: palette.siderBg,
        headerBg: '#FFFFFF',
        bodyBg: SEMANTIC_COLORS.bgLayout,
        footerBg: '#FFFFFF',
      },
      Menu: {
        itemHeight: 40,
        darkItemBg: palette.siderBg,
        darkSubMenuItemBg: palette.siderSubBg,
        darkItemColor: mixHex(palette.siderText, palette.siderBg, 0.25),
        darkItemHoverColor: palette.siderText,
        darkItemSelectedBg: palette.primary,
        darkItemSelectedColor: '#FFFFFF',
        darkGroupTitleColor: mixHex(palette.siderText, palette.siderBg, 0.45),
      },
      Button: { primaryShadow: 'none', defaultShadow: 'none', dangerShadow: 'none' },
      Card: { headerFontSize: 16 },
      Table: { headerBg: SEMANTIC_COLORS.tableHeaderBg, headerSplitColor: 'transparent' },
      Tag: { defaultBg: SEMANTIC_COLORS.tableHeaderBg },
    },
  };
}

const PRESET_STORAGE_KEY = 'hammer.dev.themePreset';

/**
 * 開發模式下選用的品牌色組：網址的 `?theme=navy|teal|<6 碼色碼>` 優先，其次是開發用下拉選單
 * 存在本機的選擇。正式版一律使用 {@link DEFAULT_PRESET}。
 */
export function resolvePalette(dev: boolean, search: string, stored: string | null): BrandPalette {
  if (!dev) {
    return THEME_PRESETS[DEFAULT_PRESET];
  }
  const choice = new URLSearchParams(search).get('theme') ?? stored;
  if (choice && choice in THEME_PRESETS) {
    return THEME_PRESETS[choice as ThemePresetKey];
  }
  const hex = choice ? normalizeHex(choice) : null;
  return hex ? customPalette(hex) : THEME_PRESETS[DEFAULT_PRESET];
}

export function loadStoredPreset(): string | null {
  try {
    return localStorage.getItem(PRESET_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function saveStoredPreset(value: string) {
  try {
    localStorage.setItem(PRESET_STORAGE_KEY, value);
  } catch {
    // 無法寫入本機儲存時，只影響開發用的切換器
  }
}

/** 目前使用的品牌色組（main.tsx 啟動時決定；測試使用預設）。 */
export const appPalette: BrandPalette =
  typeof window === 'undefined'
    ? THEME_PRESETS[DEFAULT_PRESET]
    : resolvePalette(
        import.meta.env.DEV && import.meta.env.MODE !== 'test',
        window.location.search,
        loadStoredPreset(),
      );

export const appTheme: ThemeConfig = buildTheme(appPalette);
