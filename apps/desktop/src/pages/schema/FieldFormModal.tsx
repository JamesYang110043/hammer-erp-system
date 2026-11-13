import { Alert, Form, Input, InputNumber, Modal, Select, Switch, Typography } from 'antd';
import { useState } from 'react';

import { useCreateField, useUpdateField } from '../../api/schema';
import type { FieldDefinition } from '../../bindings';
import { duplicateValues, formatValue } from '../../schema/errorDetails';
import type { DuplicateValue } from '../../schema/errorDetails';
import {
  NEW_FIELD_VALUES,
  toCreateRequest,
  toFormValues,
  toUpdateRequest,
} from '../../schema/fieldForm';
import type { FieldFormValues } from '../../schema/fieldForm';
import {
  DATA_TYPE_LABELS,
  SUPPORTED_DATA_TYPES,
  isPropEditable,
  isPropVisible,
} from '../../schema/fieldSettings';
import type { FieldProp } from '../../schema/fieldSettings';
import { keyRule } from '../../schema/keyRules';
import { applyFormError } from '../users/formErrors';
import { DefaultValueInput, TypeSettingsFields } from './FieldInputs';

/** `'new'` 代表新增欄位；`undefined` 代表關閉。 */
export type FieldFormTarget = FieldDefinition | 'new' | undefined;

const SWITCHES: { prop: FieldProp & keyof FieldFormValues; label: string; extra?: string }[] = [
  { prop: 'is_required', label: '必填' },
  { prop: 'is_visible', label: '顯示', extra: '關閉後不出現在列表與表單' },
  { prop: 'is_readonly', label: '唯讀', extra: '只能由系統或預設值寫入' },
  { prop: 'is_searchable', label: '可搜尋', extra: '建立索引，加快搜尋' },
  { prop: 'is_unique', label: '不可重複', extra: '已有重複的資料時無法開啟' },
];

/**
 * 新增或編輯欄位。
 *
 * - 依資料型態顯示對應的設定（{@link TypeSettingsFields}）
 * - 編輯時欄位代碼與資料型態不可修改（型態另有「變更型態」）
 * - 系統欄位只顯示可以修改的屬性（docs/architecture.md §7.2）
 */
export function FieldFormModal({
  entityKey,
  target,
  nextSortOrder,
  onClose,
}: {
  entityKey: string;
  target: FieldFormTarget;
  /** 新增欄位的排序（排在最後）。 */
  nextSortOrder: number;
  onClose: () => void;
}) {
  const [form] = Form.useForm<FieldFormValues>();
  const createField = useCreateField(entityKey);
  const updateField = useUpdateField(entityKey);
  const [error, setError] = useState<string>();
  const [duplicates, setDuplicates] = useState<{ total: number; values: DuplicateValue[] }>();

  const field = target === 'new' ? undefined : target;
  const dataType = Form.useWatch('data_type', form) ?? field?.data_type ?? 'text';
  const options = Form.useWatch('options', form) ?? [];
  const decimalPlaces = Form.useWatch('decimal_places', form);
  const pending = createField.isPending || updateField.isPending;

  const visible = (prop: FieldProp) => isPropVisible(field, prop);
  const editable = (prop: FieldProp) => isPropEditable(field, prop);

  const close = () => {
    setError(undefined);
    setDuplicates(undefined);
    createField.reset();
    updateField.reset();
    onClose();
  };

  const onError = (err: unknown) => {
    const dup = duplicateValues(err);
    setDuplicates(dup);
    setError(dup ? undefined : applyFormError(form, err));
  };

  const submit = (values: FieldFormValues) => {
    setError(undefined);
    setDuplicates(undefined);
    if (field) {
      updateField.mutate(
        { fieldKey: field.field_key, req: toUpdateRequest(field, values) },
        { onSuccess: close, onError },
      );
    } else {
      createField.mutate(toCreateRequest(values, nextSortOrder), { onSuccess: close, onError });
    }
  };

  const typeOptions = (
    field && !SUPPORTED_DATA_TYPES.includes(field.data_type)
      ? [...SUPPORTED_DATA_TYPES, field.data_type]
      : SUPPORTED_DATA_TYPES
  ).map((t) => ({ value: t, label: DATA_TYPE_LABELS[t] }));

  return (
    <Modal
      title={field ? `編輯欄位：${field.label}` : '新增欄位'}
      open={target !== undefined}
      onCancel={close}
      onOk={() => form.submit()}
      okText="儲存"
      okButtonProps={{ loading: pending }}
      destroyOnHidden
      width={640}
    >
      {target !== undefined && (
        <Form
          form={form}
          layout="vertical"
          onFinish={submit}
          disabled={pending}
          initialValues={field ? toFormValues(field) : NEW_FIELD_VALUES}
          onValuesChange={(changed: Partial<FieldFormValues>) => {
            // 換型態後原本的預設值格式不同，清除
            if ('data_type' in changed) {
              form.setFieldsValue({ default_value: undefined });
            }
          }}
          preserve={false}
        >
          {field?.is_system && (
            <Alert
              showIcon
              type="info"
              message="系統欄位只能修改顯示名稱、顯示、必填、預設值、欄寬與說明文字"
              style={{ marginBottom: 16 }}
            />
          )}
          <Form.Item
            label="欄位代碼"
            name="field_key"
            rules={editable('field_key') ? [keyRule] : []}
            extra={
              editable('field_key')
                ? '小寫英文字母開頭，只能使用小寫英文字母、數字與底線；建立後不可修改'
                : undefined
            }
          >
            <Input autoComplete="off" disabled={!editable('field_key')} />
          </Form.Item>
          <Form.Item
            label="顯示名稱"
            name="label"
            rules={[{ required: true, whitespace: true, message: '請輸入顯示名稱' }]}
          >
            <Input autoComplete="off" />
          </Form.Item>
          <Form.Item
            label="資料型態"
            name="data_type"
            extra={field && !field.is_system ? '要變更型態請使用欄位列表的「變更型態」' : undefined}
          >
            <Select options={typeOptions} disabled={!editable('data_type')} />
          </Form.Item>

          {visible('type_settings') && (
            <TypeSettingsFields dataType={dataType} disabled={!editable('type_settings')} />
          )}

          {visible('default_value') && (
            <Form.Item label="預設值" name="default_value">
              <DefaultValueInput
                dataType={dataType}
                options={options}
                decimalPlaces={decimalPlaces}
                disabled={!editable('default_value')}
              />
            </Form.Item>
          )}

          <div style={{ display: 'flex', flexWrap: 'wrap', columnGap: 24 }}>
            {SWITCHES.filter((s) => visible(s.prop)).map((s) => (
              <Form.Item
                key={s.prop}
                label={s.label}
                name={s.prop}
                valuePropName="checked"
                tooltip={s.extra}
              >
                <Switch disabled={!editable(s.prop)} />
              </Form.Item>
            ))}
          </div>

          {visible('column_width') && (
            <Form.Item label="欄寬" name="column_width" extra="列表的欄寬（px），留空為自動">
              <InputNumber min={1} precision={0} />
            </Form.Item>
          )}
          {visible('help_text') && (
            <Form.Item label="說明文字" name="help_text">
              <Input.TextArea rows={2} />
            </Form.Item>
          )}

          {duplicates && <DuplicateAlert {...duplicates} />}
          {error && <Alert showIcon type="error" message={error} />}
        </Form>
      )}
    </Modal>
  );
}

/** 無法開啟「不可重複」：列出重複的值與筆數。 */
function DuplicateAlert({ total, values }: { total: number; values: DuplicateValue[] }) {
  return (
    <Alert
      showIcon
      type="error"
      message={`已有 ${total} 個值重複，無法設為不可重複`}
      description={
        <ul style={{ margin: 0, paddingInlineStart: 20 }}>
          {values.map((v) => (
            <li key={formatValue(v.value)}>
              <Typography.Text code>{formatValue(v.value)}</Typography.Text>：{v.count} 筆
            </li>
          ))}
        </ul>
      }
    />
  );
}
