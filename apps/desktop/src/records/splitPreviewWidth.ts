// 沒有其他相依，E2E 測試也可以直接匯入

/** 分割檢視左側列表選擇器的寬度（px）。 */
export const SPLIT_PICKER_WIDTH = 320;
/** 分割檢視左右兩側的間距（px，`.master-split` 的 `gap`）。 */
export const SPLIT_GAP = 16;
/** 分割檢視右側卡片的內距（px，antd Card 預設）；表單內容寬度為卡片寬度減去左右內距。 */
export const SPLIT_DETAIL_PADDING = 24;

/**
 * 分割檢視右側（`.master-split-detail` 卡片，含內距）在 1440px 視窗的寬度（px）：
 * 1440 − 側邊欄 216 − 內容區左右內距 24×2 − 左側列表 320 − 間距 16。
 */
export const SPLIT_DETAIL_PREVIEW_WIDTH = 840;

/**
 * 分割檢視右側卡片的寬度：頁面（`.page`，內容區扣除內距後的寬度）減去左側列表與間距。
 * 設計器以目前視窗下的這個寬度預覽，與實際的右側相同（#260）。
 */
export function splitDetailWidth(pageWidth: number): number {
  return Math.max(0, pageWidth - SPLIT_PICKER_WIDTH - SPLIT_GAP);
}

/**
 * 區塊在寬度 `width`（`.form-layout-container` 的寬度）下實際顯示的欄數，與 `styles.css` 的
 * container query 相同：4 欄 ≤ 960px 為 2 欄、3 欄 ≤ 720px 為 2 欄、2–4 欄 ≤ 480px 為 1 欄。
 */
export function displayedColumns(columns: number, width: number): number {
  if (columns >= 2 && width <= 480) {
    return 1;
  }
  if ((columns === 4 && width <= 960) || (columns === 3 && width <= 720)) {
    return 2;
  }
  return columns;
}
