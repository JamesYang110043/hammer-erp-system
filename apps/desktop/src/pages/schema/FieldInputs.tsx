import { DeleteOutlined, PlusOutlined } from '@ant-design/icons';
import { Button, DatePicker, Form, Input, InputNumber, Select, Space } from 'antd';

import { FULL_ROW } from '../../components/FormGrid';
import { useEntities } from '../../api/schema';
import type { DataType, EntityDefinition } from '../../bindings';
import { MASTER_DATA_LABEL } from '../../schema/entities';
import type { OptionFormValue } from '../../schema/fieldForm';
import { MAX_DECIMAL_PLACES, isRelationTarget, typeSettings } from '../../schema/fieldSettings';

/** 選項標籤可用的顏色（Ant Design Tag 的預設顏色）。 */
const OPTION_COLORS = [
  { value: 'blue', label: '藍' },
  { value: 'green', label: '綠' },
  { value: 'gold', label: '黃' },
  { value: 'orange', label: '橘' },
  { value: 'red', label: '紅' },
  { value: 'purple', label: '紫' },
  { value: 'cyan', label: '青' },
  { value: 'magenta', label: '洋紅' },
];

/**
 * 依資料型態顯示的設定（小數位數、選項、驗證規則），見 {@link typeSettings}。
 *
 * 必須放在 antd `Form` 內；欄位名稱與 `FieldFormValues` 相同。
 */
export function TypeSettingsFields({
  dataType,
  resultType,
  disabled = false,
  relationLocked = false,
}: {
  dataType: DataType;
  /** `formula` 的結果型態（決定是否需要小數位數）。 */
  resultType?: DataType | null;
  disabled?: boolean;
  /** 關聯的資料類別不可修改（編輯既有欄位）。 */
  relationLocked?: boolean;
}) {
  const settings = typeSettings(dataType, resultType);
  return (
    <>
      {settings.includes('relation_entity') && (
        <RelationTargetField disabled={disabled || relationLocked} />
      )}
      {settings.includes('decimal_places') && (
        <Form.Item
          label="小數位數"
          name="decimal_places"
          rules={[{ required: true, message: '請輸入小數位數' }]}
        >
          <InputNumber min={0} max={MAX_DECIMAL_PLACES} precision={0} disabled={disabled} />
        </Form.Item>
      )}
      {settings.includes('options') && <OptionsField disabled={disabled} />}
      {settings.includes('min_max') && (
        <div style={{ display: 'flex', gap: 16 }}>
          <Form.Item label="最小值" name="min" style={{ flex: 1 }}>
            <InputNumber<string> stringMode style={{ width: '100%' }} disabled={disabled} />
          </Form.Item>
          <Form.Item label="最大值" name="max" style={{ flex: 1 }}>
            <InputNumber<string> stringMode style={{ width: '100%' }} disabled={disabled} />
          </Form.Item>
        </div>
      )}
      {settings.includes('max_length') && (
        <Form.Item label="字元數上限" name="max_length">
          <InputNumber min={1} precision={0} disabled={disabled} />
        </Form.Item>
      )}
      {settings.includes('pattern') && (
        <Form.Item
          label="格式規則"
          name="pattern"
          extra="正規表示式，例如 ^\d{8}$ 代表 8 位數字；不需要時留空"
        >
          <Input autoComplete="off" disabled={disabled} />
        </Form.Item>
      )}
    </>
  );
}

/** 關聯的資料類別（`relation_entity_id`）。建立後不可修改。 */
function RelationTargetField({ disabled }: { disabled: boolean }) {
  const entities = useEntities();
  return (
    <Form.Item
      label="關聯的資料類別"
      name="relation_entity_id"
      rules={[{ required: true, message: '請選擇關聯的資料類別' }]}
      extra={disabled ? undefined : '建立後不可修改'}
    >
      <Select
        disabled={disabled}
        loading={entities.isLoading}
        placeholder="選擇資料類別"
        showSearch
        optionFilterProp="label"
        options={relationTargetOptions(entities.data ?? [])}
      />
    </Form.Item>
  );
}

/**
 * 關聯的資料類別選項：分成「基本資料」（系統資料類別，例如商品、客戶）與「自訂資料類別」兩組，
 * 沒有資料類別的組不列出。
 */
function relationTargetOptions(entities: readonly EntityDefinition[]) {
  const targets = entities.filter(isRelationTarget);
  return [
    { label: MASTER_DATA_LABEL, system: true },
    { label: '自訂資料類別', system: false },
  ].flatMap(({ label, system }) => {
    const options = targets
      .filter((e) => e.is_system === system)
      .map((e) => ({ value: e.id, label: e.label }));
    return options.length > 0 ? [{ label, title: label, options }] : [];
  });
}

/** 下拉選項的清單（值、名稱、顏色）。 */
function OptionsField({ disabled }: { disabled: boolean }) {
  return (
    <Form.Item className={FULL_ROW} label="選項" required>
      <Form.List
        name="options"
        rules={[
          {
            validator: (_, options: OptionFormValue[] | undefined) =>
              options && options.length > 0
                ? Promise.resolve()
                : Promise.reject(new Error('請至少新增一個選項')),
          },
        ]}
      >
        {(fields, { add, remove }, { errors }) => (
          <>
            {fields.map(({ key, name }) => (
              <Space key={key} align="baseline" style={{ display: 'flex' }}>
                <Form.Item
                  name={[name, 'value']}
                  rules={[{ required: true, whitespace: true, message: '請輸入值' }]}
                >
                  <Input placeholder="值" aria-label="選項的值" disabled={disabled} />
                </Form.Item>
                <Form.Item
                  name={[name, 'label']}
                  rules={[{ required: true, whitespace: true, message: '請輸入名稱' }]}
                >
                  <Input placeholder="名稱" aria-label="選項的名稱" disabled={disabled} />
                </Form.Item>
                <Form.Item name={[name, 'color']}>
                  <Select
                    placeholder="顏色"
                    aria-label="選項的顏色"
                    allowClear
                    options={OPTION_COLORS}
                    style={{ width: 90 }}
                    disabled={disabled}
                  />
                </Form.Item>
                <Button
                  type="text"
                  icon={<DeleteOutlined />}
                  aria-label="移除選項"
                  onClick={() => remove(name)}
                  disabled={disabled}
                />
              </Space>
            ))}
            <Button
              type="dashed"
              icon={<PlusOutlined />}
              onClick={() => add({ value: '', label: '' })}
              disabled={disabled}
            >
              新增選項
            </Button>
            <Form.ErrorList errors={errors} />
          </>
        )}
      </Form.List>
    </Form.Item>
  );
}

/** 預設值的輸入元件，依資料型態不同（值的格式見 `FieldFormValues.default_value`）。 */
export function DefaultValueInput({
  dataType,
  options,
  decimalPlaces,
  disabled,
  value,
  onChange,
  id,
}: {
  dataType: DataType;
  options: OptionFormValue[];
  decimalPlaces?: number | null;
  disabled?: boolean;
  value?: unknown;
  onChange?: (value: unknown) => void;
  id?: string;
}) {
  const common = { id, disabled, allowClear: true } as const;
  const selectOptions = options
    .filter((o) => o.value)
    .map((o) => ({ value: o.value!, label: o.label || o.value! }));

  switch (dataType) {
    case 'long_text':
      return (
        <Input.TextArea
          {...common}
          rows={2}
          value={value as string | undefined}
          onChange={(e) => onChange?.(e.target.value)}
        />
      );
    case 'integer':
      return (
        <InputNumber<number>
          id={id}
          disabled={disabled}
          precision={0}
          value={value as number | undefined}
          onChange={(v) => onChange?.(v)}
        />
      );
    case 'decimal':
    case 'money':
      return (
        <InputNumber<string>
          id={id}
          disabled={disabled}
          stringMode
          precision={decimalPlaces ?? undefined}
          value={value as string | undefined}
          onChange={(v) => onChange?.(v)}
        />
      );
    case 'date':
    case 'datetime':
      return (
        <DatePicker
          {...common}
          showTime={dataType === 'datetime'}
          value={(value as never) ?? null}
          onChange={(v) => onChange?.(v ?? undefined)}
        />
      );
    case 'boolean':
      return (
        <Select
          {...common}
          placeholder="不設定"
          style={{ width: 120 }}
          options={[
            { value: true, label: '勾選' },
            { value: false, label: '不勾選' },
          ]}
          value={value as boolean | undefined}
          onChange={(v) => onChange?.(v)}
        />
      );
    case 'select':
    case 'multi_select':
      return (
        <Select
          {...common}
          mode={dataType === 'multi_select' ? 'multiple' : undefined}
          placeholder="不設定"
          options={selectOptions}
          value={value as string | string[] | undefined}
          onChange={(v) => onChange?.(v)}
        />
      );
    default:
      return (
        <Input
          {...common}
          autoComplete="off"
          value={value as string | undefined}
          onChange={(e) => onChange?.(e.target.value)}
        />
      );
  }
}
