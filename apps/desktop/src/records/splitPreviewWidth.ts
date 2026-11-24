// 沒有其他相依，E2E 測試也可以直接匯入

/**
 * 分割檢視右側（`.master-split-detail` 卡片，含內距）在 1440px 視窗的寬度（px）：
 * 1440 − 側邊欄 216 − 內容區左右內距 24×2 − 左側列表 320 − 間距 16。
 * 設計器以這個寬度預覽「分割檢視右側」。
 */
export const SPLIT_DETAIL_PREVIEW_WIDTH = 840;
