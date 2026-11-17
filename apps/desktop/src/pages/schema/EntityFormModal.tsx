import { Alert, Form, Input, Modal, Radio, Select, Space, Typography } from 'antd';
import type { FormRule } from 'antd';
import { useState } from 'react';

import { useCreateEntity, useUpdateEntity } from '../../api/schema';
import type { EntityDefinition, EntityDetail, EntityKind } from '../../bindings';
import { KIND_LABELS, blankToNull, toUpdateEntityRequest } from '../../schema/entities';
import type { MenuKind } from '../../schema/entities';
import { keyRule } from '../../schema/keyRules';
import { numberingExample } from '../../schema/numbering';
import { applyFormError } from '../users/formErrors';
import { ENTITY_ICONS } from './entityIcons';

/**
 * 表單的對象：
 *
 * - `{ mode: 'create' }`：新增主檔或單據
 * - `{ mode: 'create', parent }`：新增 `parent` 的明細子表
 * - `{ mode: 'edit', entity }`：修改資料類別
 */
export type EntityFormTarget =
  | {
      mode: 'create';
      parent?: EntityDefinition;
      /** 新資料類別的選單排序（排在同一組的最後面）。 */
      sortOrderFor: (kind: EntityKind) => number;
    }
  | { mode: 'edit'; entity: EntityDefinition };

interface FormValues {
  key?: string;
  label: string;
  icon?: string | null;
  kind?: MenuKind;
  numbering_pattern?: string | null;
  title_template?: string | null;
}

/** 單號規則的即時檢查（與伺服器相同的規則，伺服器仍會檢查）。 */
const numberingRule: FormRule = {
  validator: (_, value: string | undefined) => {
    const result = numberingExample(value);
    return result && !result.ok ? Promise.reject(new Error(result.message)) : Promise.resolve();
  },
};

/** 單號規則欄位下方的說明：合法時顯示範例單號。 */
function NumberingHint({ pattern }: { pattern: string | null | undefined }) {
  const result = numberingExample(pattern);
  if (!result) {
    return (
      <>
        空白代表不使用單號。可用代碼：{'{yyyy} {yy} {MM} {dd}'}，流水號 {'{0000}'}
      </>
    );
  }
  if (!result.ok) {
    return null;
  }
  return (
    <span aria-live="polite">
      範例單號：<Typography.Text code>{result.example}</Typography.Text>
    </span>
  );
}

/** 新增或修改資料類別（需要 `schema.manage`）。 */
export function EntityFormModal({
  target,
  onClose,
  onCreated,
}: {
  target: EntityFormTarget | undefined;
  onClose: () => void;
  onCreated?: (detail: EntityDetail) => void;
}) {
  const [form] = Form.useForm<FormValues>();
  const createEntity = useCreateEntity();
  const updateEntity = useUpdateEntity();
  const [error, setError] = useState<string>();
  const pattern = Form.useWatch('numbering_pattern', form);

  const editing = target?.mode === 'edit' ? target.entity : undefined;
  const parent = target?.mode === 'create' ? target.parent : undefined;
  const isDetail = editing ? editing.kind === 'detail' : parent !== undefined;
  const pending = createEntity.isPending || updateEntity.isPending;

  const close = () => {
    setError(undefined);
    createEntity.reset();
    updateEntity.reset();
    onClose();
  };

  const onError = (err: unknown) => setError(applyFormError(form, err));

  const submit = (values: FormValues) => {
    setError(undefined);
    const common = {
      label: values.label.trim(),
      icon: values.icon ?? null,
      // 系統資料類別不可設定單號規則（§7.1）
      numbering_pattern:
        isDetail || editing?.is_system ? null : blankToNull(values.numbering_pattern),
      title_template: blankToNull(values.title_template),
    };
    if (editing) {
      updateEntity.mutate(
        { key: editing.key, req: toUpdateEntityRequest(editing, common) },
        { onSuccess: close, onError },
      );
    } else if (target?.mode === 'create') {
      const kind: EntityKind = parent ? 'detail' : (values.kind ?? 'master');
      createEntity.mutate(
        {
          ...common,
          key: values.key ?? '',
          kind,
          parent_entity_id: parent?.id ?? null,
          sort_order: target.sortOrderFor(kind),
        },
        {
          onSuccess: (detail) => {
            close();
            onCreated?.(detail);
          },
          onError,
        },
      );
    }
  };

  const title = editing
    ? `編輯資料類別：${editing.label}`
    : parent
      ? `新增明細子表：${parent.label}`
      : '新增資料類別';

  const initialValues: FormValues = editing
    ? {
        label: editing.label,
        icon: editing.icon ?? undefined,
        numbering_pattern: editing.numbering_pattern ?? '',
        title_template: editing.title_template ?? '',
      }
    : { label: '', kind: 'master', numbering_pattern: '', title_template: '' };

  return (
    <Modal
      title={title}
      open={target !== undefined}
      onCancel={close}
      onOk={() => form.submit()}
      okText={editing ? '儲存' : '建立'}
      okButtonProps={{ loading: pending }}
      destroyOnHidden
    >
      <Form
        form={form}
        layout="vertical"
        onFinish={submit}
        disabled={pending}
        preserve={false}
        initialValues={initialValues}
      >
        {editing ? (
          <Form.Item label="代碼">
            <Input value={editing.key} disabled />
          </Form.Item>
        ) : (
          <Form.Item
            label="代碼"
            name="key"
            rules={[keyRule]}
            extra="小寫英文字母開頭，只能使用小寫英文字母、數字與底線；建立後不可修改"
          >
            <Input
              autoComplete="off"
              placeholder={parent ? '例如 repair_line' : '例如 repair_order'}
            />
          </Form.Item>
        )}
        <Form.Item
          label="名稱"
          name="label"
          rules={[{ required: true, whitespace: true, message: '請輸入名稱' }]}
        >
          <Input autoComplete="off" placeholder={parent ? '例如 維修明細' : '例如 維修單'} />
        </Form.Item>
        <Form.Item label="圖示" name="icon">
          <Select
            allowClear
            placeholder="預設圖示"
            options={Object.entries(ENTITY_ICONS).map(([name, icon]) => ({
              value: name,
              label: (
                <Space>
                  {icon}
                  {name}
                </Space>
              ),
            }))}
          />
        </Form.Item>
        {editing || parent ? (
          <Form.Item label="種類">
            <Input
              value={
                parent
                  ? `${KIND_LABELS.detail}（屬於「${parent.label}」）`
                  : KIND_LABELS[editing!.kind]
              }
              disabled
            />
          </Form.Item>
        ) : (
          <Form.Item
            label="種類"
            name="kind"
            extra="單據與主檔的功能相同，差別在選單中分組顯示；建立後不可修改"
          >
            <Radio.Group
              optionType="button"
              options={[
                { value: 'master', label: KIND_LABELS.master },
                { value: 'document', label: KIND_LABELS.document },
              ]}
            />
          </Form.Item>
        )}
        {!isDetail && !editing?.is_system && (
          <Form.Item
            label="單號規則"
            name="numbering_pattern"
            rules={[numberingRule]}
            extra={<NumberingHint pattern={pattern} />}
          >
            <Input autoComplete="off" placeholder="例如 SO{yyyyMM}-{0000}" />
          </Form.Item>
        )}
        <Form.Item
          label="標題範本"
          name="title_template"
          extra={
            editing
              ? '關聯欄位與選擇器顯示的文字，{} 內可寫 record_no（單號）、欄位代碼，或「關聯欄位.目標欄位」引用關聯資料的欄位，例如 {record_no} {customer_id.name}；空白使用預設規則'
              : '關聯欄位與選擇器顯示的文字；新的資料類別只能引用 {record_no}（單號），新增欄位後再回來設定；空白使用預設規則'
          }
        >
          <Input autoComplete="off" placeholder="例如 {record_no} {name}" />
        </Form.Item>
        {error && <Alert showIcon type="error" message={error} />}
      </Form>
    </Modal>
  );
}
