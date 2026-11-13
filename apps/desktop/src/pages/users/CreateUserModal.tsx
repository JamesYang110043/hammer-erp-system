import { Alert, Form, Input, Modal, Select } from 'antd';
import { useState } from 'react';

import { errorMessage } from '../../api/client';
import { useCreateUser, useRoles } from '../../api/users';
import { passwordRule } from '../../auth/passwordRules';
import type { User } from '../../bindings';
import { generateTempPassword } from '../../utils/tempPassword';
import { PasswordHandoff } from './PasswordHandoff';
import { TempPasswordInput } from './TempPasswordInput';
import { applyFormError } from './formErrors';

interface FormValues {
  username: string;
  display_name: string;
  role: string;
  password: string;
}

/** 新增帳號。成功後顯示臨時密碼（使用者第一次登入必須修改）。 */
export function CreateUserModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [form] = Form.useForm<FormValues>();
  const roles = useRoles();
  const createUser = useCreateUser();
  const [error, setError] = useState<string>();
  const [created, setCreated] = useState<{ user: User; password: string }>();

  const alert = error ?? (roles.isError ? errorMessage(roles.error) : undefined);

  const close = () => {
    setError(undefined);
    setCreated(undefined);
    createUser.reset();
    onClose();
  };

  const submit = (values: FormValues) => {
    setError(undefined);
    createUser.mutate(values, {
      onSuccess: (user) => setCreated({ user, password: values.password }),
      onError: (err) => setError(applyFormError(form, err)),
    });
  };

  return (
    <Modal
      title="新增帳號"
      open={open}
      onCancel={close}
      destroyOnHidden
      okText={created ? '完成' : '建立'}
      onOk={created ? close : () => form.submit()}
      cancelButtonProps={{ style: created ? { display: 'none' } : undefined }}
      okButtonProps={{ loading: createUser.isPending }}
    >
      {created ? (
        <PasswordHandoff
          title="帳號已建立"
          username={created.user.username}
          password={created.password}
        />
      ) : (
        <Form
          form={form}
          layout="vertical"
          onFinish={submit}
          disabled={createUser.isPending}
          initialValues={{ password: generateTempPassword() }}
          preserve={false}
        >
          <Form.Item
            label="帳號"
            name="username"
            rules={[{ required: true, whitespace: true, message: '請輸入帳號' }]}
          >
            <Input autoComplete="off" />
          </Form.Item>
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
          <Form.Item
            label="臨時密碼"
            name="password"
            extra="使用者第一次登入時必須修改密碼"
            rules={[{ required: true, message: '請輸入臨時密碼' }, passwordRule]}
          >
            <TempPasswordInput />
          </Form.Item>
          {alert && <Alert showIcon type="error" message={alert} />}
        </Form>
      )}
    </Modal>
  );
}
