import { theme } from 'antd';
import type { ReactNode } from 'react';

import type { FieldDefinition } from '../../bindings';
import type { FormLayoutConfig } from '../../records/formLayout';
import { FormLayoutRenderer } from './FormLayoutRenderer';

/** 唯讀預覽的一個欄位：上方標籤、下方值。 */
export function PreviewItem({ label, children }: { label: string; children: ReactNode }) {
  const { token } = theme.useToken();
  return (
    <div className="preview-item" style={{ marginBottom: token.marginSM }}>
      <div style={{ color: token.colorTextSecondary, fontSize: token.fontSizeSM, marginBottom: 2 }}>
        {label}
      </div>
      <div style={{ minHeight: token.controlHeightSM, wordBreak: 'break-word' }}>{children}</div>
    </div>
  );
}

/**
 * 依表單版面（docs/architecture.md §7.7）排列的唯讀欄位：分割檢視右側的預覽
 *（`RecordPreview`）與表單版面設計器的「唯讀預覽」共用，外觀一致。
 *
 * - 每個欄位以 {@link PreviewItem} 顯示，值由 `renderValue` 決定（實際資料或範例）
 * - 不在 `fields` 中的欄位（不顯示、已停用）不顯示
 */
export function ReadonlyLayout({
  config,
  fields,
  renderValue,
  renderBlock,
}: {
  config: FormLayoutConfig;
  /** 要顯示的欄位。 */
  fields: readonly FieldDefinition[];
  renderValue: (field: FieldDefinition) => ReactNode;
  renderBlock: (code: string) => ReactNode;
}) {
  const byKey = new Map(fields.map((f) => [f.field_key, f]));
  return (
    <FormLayoutRenderer
      config={config}
      renderField={(fieldKey) => {
        const field = byKey.get(fieldKey);
        return field ? <PreviewItem label={field.label}>{renderValue(field)}</PreviewItem> : null;
      }}
      renderBlock={renderBlock}
    />
  );
}
