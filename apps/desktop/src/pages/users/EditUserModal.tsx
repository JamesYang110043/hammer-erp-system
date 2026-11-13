import { Alert, Form, Input, Modal, Select, Switch } from 'antd';
import { useState } from 'react';

import { useRoles, useUpdateUser } from '../../api/users';
import type { UpdateUserRequest, User } from '../../bindings';
import { applyFormError } from './formErrors';

interface FormValues {
  display_name: string;
  role: string;
  is_active: boolean;
}

/** 編輯帳號的顯示名稱、角色、啟用狀態。只送出有變更的欄位，並帶上讀取時的版本號。 */
export function EditUserModal({ user, onClose }: { user: User | undefined; onClose: () => void }) {
  const [form] = Form.useForm<FormValues>();
  const roles = useRoles();
  const updateUser = useUpdateUser();
  const [error, setError] = useState<string>();

  const close = () => {
    setError(undefined);
    updateUser.reset();
    onClose();
  };

  const submit = (values: FormValues) => {
    if (!user) {
      return;
    }
    const displayName = values.display_name.trim();
    const req: UpdateUserRequest = {
      version: user.version,
      display_name: displayName !== user.display_name ? displayName : null,
      role: values.role !== user.role ? values.role : null,
      is_active: values.is_active !== user.is_active ? values.is_active : null,
    };
    setError(undefined);
    updateUser.mutate(
      { id: user.id, req },
      { onSuccess: close, onError: (err) => setError(applyFormError(form, err)) },
    );
  };

  return (
    <Modal
      title={user ? `編輯帳號：${user.username}` : '編輯帳號'}
      open={user !== undefined}
      onCancel={close}
      onOk={() => form.submit()}
      okText="儲存"
      okButtonProps={{ loading: updateUser.isPending }}
      destroyOnHidden
    >
      {user && (
        <Form
          form={form}
          layout="vertical"
          onFinish={submit}
          disabled={updateUser.isPending}
          initialValues={{
            display_name: user.display_name,
            role: user.role,
            is_active: user.is_active,
          }}
          preserve={false}
        >
          <Form.Item
            label="顯示名稱"
            name="display_name"
            rules={[{ required: true, whitespace: true, message: '請輸入顯示名稱' }]}
          >
            <Input autoComplete="off" />
          </Form.Item>
          <Form.Item label="角色" name="role" rules={[{ required: true, message: '請選擇角色' }]}>
            <Select
              loading={roles.isPending}
              options={roles.data?.map((r) => ({ value: r.code, label: r.name }))}
            />
          </Form.Item>
          <Form.Item label="啟用" name="is_active" valuePropName="checked" extra="停用後無法登入">
            <Switch />
          </Form.Item>
          {error && <Alert showIcon type="error" message={error} />}
        </Form>
      )}
    </Modal>
  );
}
