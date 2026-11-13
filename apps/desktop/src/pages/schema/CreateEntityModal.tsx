import { Alert, Form, Input, Modal, Select, Space } from 'antd';
import { useState } from 'react';

import { useCreateEntity } from '../../api/schema';
import type { EntityDetail } from '../../bindings';
import { keyRule } from '../../schema/keyRules';
import { applyFormError } from '../users/formErrors';
import { ENTITY_ICONS } from './entityIcons';

interface FormValues {
  key: string;
  label: string;
  icon?: string;
}

/** 新增自訂資料類別。M2a 只能建立主檔（`kind = master`）。 */
export function CreateEntityModal({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (detail: EntityDetail) => void;
}) {
  const [form] = Form.useForm<FormValues>();
  const createEntity = useCreateEntity();
  const [error, setError] = useState<string>();

  const close = () => {
    setError(undefined);
    createEntity.reset();
    onClose();
  };

  const submit = (values: FormValues) => {
    setError(undefined);
    createEntity.mutate(
      {
        key: values.key,
        label: values.label.trim(),
        icon: values.icon ?? null,
        kind: 'master',
        title_template: null,
        sort_order: 0,
      },
      {
        onSuccess: (detail) => {
          close();
          onCreated(detail);
        },
        onError: (err) => setError(applyFormError(form, err)),
      },
    );
  };

  return (
    <Modal
      title="新增資料類別"
      open={open}
      onCancel={close}
      onOk={() => form.submit()}
      okText="建立"
      okButtonProps={{ loading: createEntity.isPending }}
      destroyOnHidden
    >
      <Form
        form={form}
        layout="vertical"
        onFinish={submit}
        disabled={createEntity.isPending}
        preserve={false}
      >
        <Form.Item
          label="代碼"
          name="key"
          rules={[keyRule]}
          extra="小寫英文字母開頭，只能使用小寫英文字母、數字與底線；建立後不可修改"
        >
          <Input autoComplete="off" placeholder="例如 repair_order" />
        </Form.Item>
        <Form.Item
          label="名稱"
          name="label"
          rules={[{ required: true, whitespace: true, message: '請輸入名稱' }]}
        >
          <Input autoComplete="off" placeholder="例如 維修單" />
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
        <Form.Item label="類型">
          <Input value="主檔" disabled />
        </Form.Item>
        {error && <Alert showIcon type="error" message={error} />}
      </Form>
    </Modal>
  );
}
