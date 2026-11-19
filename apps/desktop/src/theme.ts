import type { ThemeConfig } from 'antd';

/**
 * 全站 antd 主題（main.tsx 與測試的 render.tsx 共用）。
 *
 * `controlHeight` 同時決定按鈕、輸入框、下拉選單、日期選擇器的高度，調整這一個值就能讓
 * 並排的按鈕與輸入框保持對齊；small / large 尺寸依比例跟著變（27 / 45）。
 * 左側選單的項目高度原本由 `controlHeightLG` 推算，固定為原本的 40，不跟著變。
 */
export const CONTROL_HEIGHT = 36;

export const appTheme: ThemeConfig = {
  token: { controlHeight: CONTROL_HEIGHT },
  components: {
    Menu: { itemHeight: 40 },
  },
};
