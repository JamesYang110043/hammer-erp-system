import { Checkbox, DatePicker, Input, Select } from 'antd';
import type { Dayjs } from 'dayjs';

import type { FieldDefinition } from '../../bindings';

/**
 * 依欄位型態的輸入元件，放在 antd `Form.Item` 內（`value`、`onChange` 由 Form 注入）。
 *
 * | 型態 | 元件 |
 * |---|---|
 * | `text` | 單行輸入框 |
 * | `long_text` | 多行輸入框 |
 * | `integer` | 輸入框（`inputMode="numeric"`） |
 * | `decimal`、`money` | 輸入框（`inputMode="decimal"`）；值為字串，不轉成浮點數（D9） |
 * | `date` | 日期選擇 |
 * | `datetime` | 日期時間選擇（台北時間） |
 * | `boolean` | 勾選框 |
 * | `select` | 下拉單選 |
 * | `multi_select` | 下拉多選 |
 */
export function RecordFieldInput({
  field,
  disabled,
  value,
  onChange,
  id,
}: {
  field: FieldDefinition;
  disabled?: boolean;
  value?: unknown;
  onChange?: (value: unknown) => void;
  id?: string;
}) {
  const common = { id, disabled, 'aria-label': field.label };
  const text = typeof value === 'string' ? value : '';
  const options = (field.options ?? []).map((o) => ({ value: o.value, label: o.label }));

  switch (field.data_type) {
    case 'long_text':
      return (
        <Input.TextArea
          {...common}
          autoSize={{ minRows: 3, maxRows: 10 }}
          value={text}
          onChange={(e) => onChange?.(e.target.value)}
        />
      );
    case 'integer':
      return (
        <Input
          {...common}
          inputMode="numeric"
          autoComplete="off"
          style={{ maxWidth: 240 }}
          value={text}
          onChange={(e) => onChange?.(e.target.value)}
        />
      );
    case 'decimal':
    case 'money':
      return (
        <Input
          {...common}
          inputMode="decimal"
          autoComplete="off"
          style={{ maxWidth: 240 }}
          placeholder={field.decimal_places ? `最多 ${field.decimal_places} 位小數` : undefined}
          value={text}
          onChange={(e) => onChange?.(e.target.value)}
        />
      );
    case 'date':
      return (
        <DatePicker
          {...common}
          value={(value as Dayjs | undefined) ?? null}
          onChange={(d) => onChange?.(d ?? undefined)}
        />
      );
    case 'datetime':
      return (
        <DatePicker
          {...common}
          showTime={{ format: 'HH:mm' }}
          format="YYYY-MM-DD HH:mm"
          value={(value as Dayjs | undefined) ?? null}
          onChange={(d) => onChange?.(d ?? undefined)}
        />
      );
    case 'boolean':
      return (
        <Checkbox
          {...common}
          checked={value === true}
          onChange={(e) => onChange?.(e.target.checked)}
        />
      );
    case 'select':
      return (
        <Select
          {...common}
          allowClear
          style={{ maxWidth: 320 }}
          value={text || undefined}
          onChange={(v: string | undefined) => onChange?.(v)}
          options={options}
        />
      );
    case 'multi_select':
      return (
        <Select
          {...common}
          mode="multiple"
          allowClear
          style={{ maxWidth: 480 }}
          value={Array.isArray(value) ? value : []}
          onChange={(v: string[]) => onChange?.(v)}
          options={options}
        />
      );
    default:
      return (
        <Input
          {...common}
          autoComplete="off"
          value={text}
          onChange={(e) => onChange?.(e.target.value)}
        />
      );
  }
}
