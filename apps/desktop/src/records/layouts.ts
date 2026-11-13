import type { ViewLayout_Serialize as ViewLayout } from '../bindings';
import type { LayoutConfig } from './columns';

export type { ViewLayout };

/** 版面切換選單中「依欄位定義」的值（沒有使用任何已儲存的版面）。 */
export const GENERATED_LAYOUT_ID = '';

/** 兩個版面設定是否相同（用來判斷目前的畫面是否與版面不同）。 */
export function sameConfig(a: LayoutConfig, b: LayoutConfig): boolean {
  return JSON.stringify(normalize(a)) === JSON.stringify(normalize(b));
}

function normalize(config: LayoutConfig) {
  return {
    columns: config.columns.map((c) => [c.field_key, c.width ?? null]),
    sort: config.sort ? [config.sort.field_key, config.sort.descending] : null,
    filters: config.filters.map((f) => [f.field_key, f.op, f.value]),
  };
}

/**
 * 目前使用者是否可以修改、刪除這個版面（只用來顯示或隱藏按鈕，以伺服器檢查為準）。
 *
 * - 全域版面：需要 `schema.manage`
 * - 個人版面：只有本人
 */
export function canEditLayout(
  layout: ViewLayout,
  userId: string | undefined,
  canManageSchema: boolean,
): boolean {
  return layout.user_id === null ? canManageSchema : layout.user_id === userId;
}
