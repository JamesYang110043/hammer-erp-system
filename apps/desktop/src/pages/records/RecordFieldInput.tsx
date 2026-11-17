import { Checkbox, DatePicker, Input, Select } from 'antd';
import type { Dayjs } from 'dayjs';

import type { Attachment } from '../../api/attachments';
import { useEntities } from '../../api/schema';
import type { FieldDefinition, RelationRef } from '../../bindings';
import { CATEGORY_ENTITY_KEY, isCurrencyField } from '../../records/masterData';
import { CategorySelect } from './CategorySelect';
import { CurrencySelect } from './CurrencySelect';
import { FileInput } from './FileInput';
import { RelationSelect } from './RelationSelect';

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
 * | `relation` | 可搜尋的下拉選單（{@link RelationSelect}）；指向商品分類時顯示階層路徑（{@link CategorySelect}） |
 * | `file` | 上傳、下載、移除（{@link FileInput}） |
 *
 * `ui_widget = currency` 的文字欄位以幣別選單輸入（{@link CurrencySelect}）。
 */
export function RecordFieldInput({
  field,
  disabled,
  value,
  onChange,
  id,
  relation,
  attachment,
}: {
  field: FieldDefinition;
  disabled?: boolean;
  value?: unknown;
  onChange?: (value: unknown) => void;
  id?: string;
  /** 資料目前的關聯（`relation` 欄位顯示標題用）。 */
  relation?: RelationRef;
  /** 資料目前的附件（`file` 欄位顯示檔名用）。 */
  attachment?: Attachment;
}) {
  const common = { id, disabled, 'aria-label': field.label };
  const text = typeof value === 'string' ? value : '';
  const options = (field.options ?? []).map((o) => ({ value: o.value, label: o.label }));

  if (isCurrencyField(field)) {
    return <CurrencySelect {...common} value={text || undefined} onChange={(v) => onChange?.(v)} />;
  }

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
    case 'relation':
      return (
        <RelationInput
          {...common}
          field={field}
          initial={relation}
          value={text || undefined}
          onChange={(v) => onChange?.(v)}
        />
      );
    case 'file':
      return (
        <FileInput
          {...common}
          initial={attachment}
          value={text || undefined}
          onChange={(v) => onChange?.(v)}
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

/** 關聯欄位：指向商品分類時用 {@link CategorySelect}，其他用 {@link RelationSelect}。 */
function RelationInput(props: Parameters<typeof RelationSelect>[0]) {
  const entities = useEntities();
  const target = entities.data?.find((e) => e.id === props.field.relation_entity_id);
  if (target?.key === CATEGORY_ENTITY_KEY) {
    return (
      <CategorySelect
        id={props.id}
        aria-label={props['aria-label']}
        disabled={props.disabled}
        style={props.style}
        initial={props.initial}
        value={props.value}
        onChange={props.onChange}
      />
    );
  }
  return <RelationSelect {...props} />;
}
