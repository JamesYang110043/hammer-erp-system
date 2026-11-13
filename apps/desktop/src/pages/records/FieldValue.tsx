import { Checkbox, Space, Tag, Typography } from 'antd';

import type { FieldDefinition } from '../../bindings';
import { findOption, formatFieldText } from '../../records/display';

function OptionTag({ field, value }: { field: FieldDefinition; value: string }) {
  const option = findOption(field, value);
  return <Tag color={option?.color ?? undefined}>{option?.label ?? value}</Tag>;
}

/**
 * 列表儲存格：依欄位型態顯示值。
 *
 * - 勾選：唯讀的勾選框
 * - 單選、多選：選項顏色標籤
 * - 其他：見 {@link formatFieldText}（金額千分位、日期時間為台北時間）
 */
export function FieldValue({ field, value }: { field: FieldDefinition; value: unknown }) {
  if (value === null || value === undefined || value === '') {
    if (field.data_type === 'boolean') {
      return <Checkbox checked={false} disabled aria-label={field.label} />;
    }
    return <Typography.Text type="secondary">—</Typography.Text>;
  }
  switch (field.data_type) {
    case 'boolean':
      return <Checkbox checked={value === true} disabled aria-label={field.label} />;
    case 'select':
      return typeof value === 'string' ? <OptionTag field={field} value={value} /> : null;
    case 'multi_select':
      return Array.isArray(value) ? (
        <Space size={[0, 4]} wrap>
          {value.map((v) => (
            <OptionTag key={String(v)} field={field} value={String(v)} />
          ))}
        </Space>
      ) : null;
    default:
      return <>{formatFieldText(field, value)}</>;
  }
}
