import { Alert, Checkbox, Form, Input, Modal } from 'antd';
import { useState } from 'react';

import { useCreateLayout } from '../../api/records';
import type { ViewLayout } from '../../bindings';
import type { LayoutConfig } from '../../records/columns';
import { applyFormError } from '../users/formErrors';

/** 版面名稱的長度上限，與 `hammer_shared::layout::MAX_LAYOUT_NAME_CHARS` 相同。 */
const MAX_LAYOUT_NAME_CHARS = 100;

interface FormValues {
  name: string;
  isGlobal: boolean;
  isDefault: boolean;
}

/**
 * 把目前的欄位、欄寬、排序與篩選另存成新版面。
 *
 * 預設勾選「設為預設」，下次開啟列表時直接套用。全域版面需要 `schema.manage`（伺服器檢查）。
 */
export function SaveLayoutModal({
  entityKey,
  open,
  config,
  canSaveGlobal,
  onClose,
  onSaved,
}: {
  entityKey: string;
  open: boolean;
  config: LayoutConfig;
  canSaveGlobal: boolean;
  onClose: () => void;
  onSaved: (layout: ViewLayout) => void;
}) {
  const [form] = Form.useForm<FormValues>();
  const createLayout = useCreateLayout(entityKey);
  const [error, setError] = useState<string>();

  const close = () => {
    setError(undefined);
    createLayout.reset();
    onClose();
  };

  const submit = (values: FormValues) => {
    setError(undefined);
    createLayout.mutate(
      {
        name: values.name.trim(),
        is_global: canSaveGlobal && values.isGlobal,
        config,
        is_default: values.isDefault,
      },
      {
        onSuccess: (layout) => {
          close();
          onSaved(layout);
        },
        onError: (err) => setError(applyFormError(form, err)),
      },
    );
  };

  return (
    <Modal
      title="另存新版面"
      open={open}
      onCancel={close}
      onOk={() => form.submit()}
      okText="儲存"
      okButtonProps={{ loading: createLayout.isPending }}
      destroyOnHidden
    >
      <Form
        form={form}
        layout="vertical"
        onFinish={submit}
        disabled={createLayout.isPending}
        preserve={false}
        initialValues={{ name: '', isGlobal: false, isDefault: true }}
      >
        <Form.Item
          label="版面名稱"
          name="name"
          rules={[
            { required: true, whitespace: true, message: '請輸入版面名稱' },
            { max: MAX_LAYOUT_NAME_CHARS, message: `最多 ${MAX_LAYOUT_NAME_CHARS} 個字` },
          ]}
        >
          <Input autoComplete="off" />
        </Form.Item>
        {canSaveGlobal && (
          <Form.Item name="isGlobal" valuePropName="checked" extra="所有使用者都可以選用">
            <Checkbox>全域版面</Checkbox>
          </Form.Item>
        )}
        <Form.Item name="isDefault" valuePropName="checked" extra="開啟列表時自動套用">
          <Checkbox>設為預設</Checkbox>
        </Form.Item>
        {error && <Alert showIcon type="error" message={error} />}
      </Form>
    </Modal>
  );
}
