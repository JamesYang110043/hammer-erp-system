import type { ReactNode } from 'react';

/** 多欄表單的 Modal 寬度（單欄 Modal 維持 antd 預設 520）。 */
export const FORM_MODAL_WIDTH = 720;

/**
 * 佔滿整列的欄位：加在 `Form.Item` 的 `className`。
 *
 * 用於長欄位（多行文字、選項清單、說明很長的欄位）。不是 `Form.Item` 的內容
 *（Alert、說明文字、自訂區塊）預設就佔滿整列，不需要加。
 */
export const FULL_ROW = 'form-grid-full';

/**
 * Modal 表單的多欄排列（樣式在 styles.css）：寬度足夠時每列 2 欄，變窄時自動退回單欄。
 *
 * 欄位依 DOM 順序由左到右、由上到下排列，Tab 鍵的移動順序與畫面一致。
 * 放在 `Form` 內，直接包住 `Form.Item`（Fragment 內的 `Form.Item` 也算）。
 */
export function FormGrid({ children }: { children: ReactNode }) {
  return (
    <div className="form-grid-container">
      <div className="form-grid">{children}</div>
    </div>
  );
}
