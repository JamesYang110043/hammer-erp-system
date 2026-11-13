import { Alert, Form, Modal, Typography } from 'antd';
import { useState } from 'react';

import { useResetUserPassword } from '../../api/users';
import { passwordRule } from '../../auth/passwordRules';
import type { User } from '../../bindings';
import { generateTempPassword } from '../../utils/tempPassword';
import { PasswordHandoff } from './PasswordHandoff';
import { TempPasswordInput } from './TempPasswordInput';
import { applyFormError } from './formErrors';

interface FormValues {
  new_password: string;
}

/** 重設密碼為臨時密碼；成功後顯示臨時密碼，提醒管理者交給使用者。 */
export function ResetPasswordModal({
  user,
  onClose,
}: {
  user: User | undefined;
  onClose: () => void;
}) {
  const [form] = Form.useForm<FormValues>();
  const resetPassword = useResetUserPassword();
  const [error, setError] = useState<string>();
  const [password, setPassword] = useState<string>();

  const close = () => {
    setError(undefined);
    setPassword(undefined);
    resetPassword.reset();
    onClose();
  };

  const submit = (values: FormValues) => {
    if (!user) {
      return;
    }
    setError(undefined);
    resetPassword.mutate(
      { id: user.id, req: { new_password: values.new_password } },
      {
        onSuccess: () => setPassword(values.new_password),
        onError: (err) => setError(applyFormError(form, err)),
      },
    );
  };

  return (
    <Modal
      title={user ? `重設密碼：${user.username}` : '重設密碼'}
      open={user !== undefined}
      onCancel={close}
      okText={password ? '完成' : '重設密碼'}
      onOk={password ? close : () => form.submit()}
      cancelButtonProps={{ style: password ? { display: 'none' } : undefined }}
      okButtonProps={{ loading: resetPassword.isPending }}
      destroyOnHidden
    >
      {user && password ? (
        <PasswordHandoff title="密碼已重設" username={user.username} password={password} />
      ) : (
        <Form
          form={form}
          layout="vertical"
          onFinish={submit}
          disabled={resetPassword.isPending}
          initialValues={{ new_password: generateTempPassword() }}
          preserve={false}
        >
          <Typography.Paragraph type="secondary">
            重設後該使用者會被登出、解除鎖定，下次登入必須修改密碼。
          </Typography.Paragraph>
          <Form.Item
            label="臨時密碼"
            name="new_password"
            rules={[{ required: true, message: '請輸入臨時密碼' }, passwordRule]}
          >
            <TempPasswordInput />
          </Form.Item>
          {error && <Alert showIcon type="error" message={error} />}
        </Form>
      )}
    </Modal>
  );
}
