import { Alert, Button, Form, Input } from 'antd';

import { errorMessage } from '../api/client';
import { passwordRule, sameAsRule } from '../auth/passwordRules';
import { useChangePassword } from '../auth/session';

interface FormValues {
  currentPassword: string;
  newPassword: string;
  confirmPassword: string;
}

/** 修改自己的密碼（強制改密碼頁與「修改密碼」對話框共用）。 */
export function ChangePasswordForm({ onSuccess }: { onSuccess?: () => void }) {
  const changePassword = useChangePassword();

  const handleFinish = (values: FormValues) => {
    changePassword.mutate(
      { current_password: values.currentPassword, new_password: values.newPassword },
      { onSuccess },
    );
  };

  return (
    <Form
      layout="vertical"
      onFinish={handleFinish}
      onValuesChange={() => changePassword.reset()}
      disabled={changePassword.isPending}
    >
      <Form.Item
        label="目前的密碼"
        name="currentPassword"
        rules={[{ required: true, message: '請輸入目前的密碼' }]}
      >
        <Input.Password autoComplete="current-password" />
      </Form.Item>
      <Form.Item
        label="新密碼"
        name="newPassword"
        rules={[{ required: true, message: '請輸入新密碼' }, passwordRule]}
      >
        <Input.Password autoComplete="new-password" />
      </Form.Item>
      <Form.Item
        label="確認新密碼"
        name="confirmPassword"
        dependencies={['newPassword']}
        rules={[{ required: true, message: '請再輸入一次新密碼' }, sameAsRule('newPassword')]}
      >
        <Input.Password autoComplete="new-password" />
      </Form.Item>
      {changePassword.isError && (
        <Form.Item>
          <Alert showIcon type="error" message={errorMessage(changePassword.error)} />
        </Form.Item>
      )}
      <Button type="primary" htmlType="submit" block loading={changePassword.isPending}>
        修改密碼
      </Button>
    </Form>
  );
}
