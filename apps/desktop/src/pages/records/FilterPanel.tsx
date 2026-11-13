import { DeleteOutlined, FilterOutlined, PlusOutlined } from '@ant-design/icons';
import { Button, DatePicker, Input, Select, Space } from 'antd';
import dayjs from 'dayjs';
import { useState } from 'react';

import type { FieldDefinition, RecordFilter } from '../../bindings';
import {
  FILTER_OP_LABELS,
  filterOps,
  fromRecordFilters,
  isValueless,
  toRecordFilters,
} from '../../records/query';
import type { FilterRow } from '../../records/query';

/** 日期時間篩選值：選擇的時間視為台北時間（D13）。 */
const DATETIME_VALUE_FORMAT = 'YYYY-MM-DDTHH:mm:ss';

function ValueInput({
  field,
  row,
  onChange,
}: {
  field: FieldDefinition;
  row: FilterRow;
  onChange: (value: string | string[] | null) => void;
}) {
  const label = `${field.label}的篩選值`;
  const text = typeof row.value === 'string' ? row.value : '';
  const options = (field.options ?? []).map((o) => ({ value: o.value, label: o.label }));

  switch (field.data_type) {
    case 'date':
      return (
        <DatePicker
          aria-label={label}
          value={text ? dayjs(text) : null}
          onChange={(d) => onChange(d ? d.format('YYYY-MM-DD') : null)}
        />
      );
    case 'datetime':
      return (
        <DatePicker
          aria-label={label}
          showTime={{ format: 'HH:mm' }}
          format="YYYY-MM-DD HH:mm"
          value={text ? dayjs(text.slice(0, 19)) : null}
          onChange={(d) => onChange(d ? `${d.format(DATETIME_VALUE_FORMAT)}+08:00` : null)}
        />
      );
    case 'boolean':
      return (
        <Select
          aria-label={label}
          style={{ width: 100 }}
          value={text || undefined}
          onChange={onChange}
          options={[
            { value: 'true', label: '是' },
            { value: 'false', label: '否' },
          ]}
        />
      );
    case 'select':
      return (
        <Select
          aria-label={label}
          style={{ width: 180 }}
          value={text || undefined}
          onChange={onChange}
          options={options}
        />
      );
    case 'multi_select':
      return (
        <Select
          aria-label={label}
          mode="multiple"
          style={{ minWidth: 180 }}
          value={Array.isArray(row.value) ? row.value : []}
          onChange={onChange}
          options={options}
        />
      );
    case 'integer':
    case 'decimal':
    case 'money':
      // 數值以字串處理，不轉成浮點數（D9）
      return (
        <Input
          aria-label={label}
          inputMode="decimal"
          style={{ width: 160 }}
          value={text}
          onChange={(e) => onChange(e.target.value)}
        />
      );
    default:
      return (
        <Input
          aria-label={label}
          style={{ width: 200 }}
          value={text}
          onChange={(e) => onChange(e.target.value)}
        />
      );
  }
}

/**
 * 篩選條件（全部符合）。依欄位型態提供對應的運算子與輸入元件。
 *
 * 編輯中的條件按「套用篩選」才生效。`filters` 改變（例如切換版面）時，父元件以 `key` 重設本元件。
 */
export function FilterPanel({
  fields,
  filters,
  onApply,
}: {
  /** 可以篩選的欄位。 */
  fields: readonly FieldDefinition[];
  filters: readonly RecordFilter[];
  onApply: (filters: RecordFilter[]) => void;
}) {
  const [rows, setRows] = useState<FilterRow[]>(() =>
    fromRecordFilters(filters).filter((r) => fields.some((f) => f.field_key === r.fieldKey)),
  );
  const byKey = new Map(fields.map((f) => [f.field_key, f]));

  const update = (index: number, patch: Partial<FilterRow>) =>
    setRows((prev) => prev.map((r, i) => (i === index ? { ...r, ...patch } : r)));

  const changeField = (index: number, fieldKey: string) => {
    const field = byKey.get(fieldKey);
    update(index, { fieldKey, op: field ? filterOps(field.data_type)[0]! : 'eq', value: null });
  };

  return (
    <div style={{ marginBottom: 16 }}>
      <Space direction="vertical" style={{ width: '100%' }}>
        {rows.map((row, index) => {
          const field = byKey.get(row.fieldKey);
          return (
            <Space key={index} wrap>
              <Select
                aria-label="篩選欄位"
                placeholder="欄位"
                style={{ width: 160 }}
                value={row.fieldKey || undefined}
                onChange={(v: string) => changeField(index, v)}
                options={fields.map((f) => ({ value: f.field_key, label: f.label }))}
              />
              {field && (
                <Select
                  aria-label="篩選條件"
                  style={{ width: 120 }}
                  value={row.op}
                  onChange={(op) => update(index, { op, value: op === 'any' ? [] : null })}
                  options={filterOps(field.data_type).map((op) => ({
                    value: op,
                    label: FILTER_OP_LABELS[op],
                  }))}
                />
              )}
              {field && !isValueless(row.op) && (
                <ValueInput
                  field={field}
                  row={row}
                  onChange={(value) => update(index, { value })}
                />
              )}
              <Button
                type="text"
                aria-label="移除條件"
                icon={<DeleteOutlined />}
                onClick={() => setRows((prev) => prev.filter((_, i) => i !== index))}
              />
            </Space>
          );
        })}
        <Space>
          <Button
            icon={<PlusOutlined />}
            onClick={() => setRows((prev) => [...prev, { fieldKey: '', op: 'eq', value: null }])}
            disabled={fields.length === 0}
          >
            新增條件
          </Button>
          <Button
            type="primary"
            icon={<FilterOutlined />}
            onClick={() => onApply(toRecordFilters(rows))}
          >
            套用篩選
          </Button>
          {(rows.length > 0 || filters.length > 0) && (
            <Button
              onClick={() => {
                setRows([]);
                onApply([]);
              }}
            >
              清除篩選
            </Button>
          )}
        </Space>
      </Space>
    </div>
  );
}
