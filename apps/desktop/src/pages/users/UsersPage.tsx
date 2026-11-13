import { PlusOutlined, ReloadOutlined } from '@ant-design/icons';
import { Alert, App, Button, Card, Popconfirm, Space, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';

import { errorMessage } from '../../api/client';
import { mutationErrorMessage } from '../../api/errors';
import { useDeleteUser, useUnlockUser, useUsers } from '../../api/users';
import type { User } from '../../bindings';
import { formatTaipei } from '../../utils/time';
import { CreateUserModal } from './CreateUserModal';
import { EditUserModal } from './EditUserModal';
import { ResetPasswordModal } from './ResetPasswordModal';

/** 帳號狀態：停用優先於鎖定。 */
function StatusTag({ user }: { user: User }) {
  if (!user.is_active) {
    return <Tag color="default">停用</Tag>;
  }
  if (user.is_locked) {
    const until = user.locked_until ? `（至 ${formatTaipei(user.locked_until, 'HH:mm')}）` : '';
    return <Tag color="error">鎖定{until}</Tag>;
  }
  return <Tag color="success">正常</Tag>;
}

/** 帳號管理頁（需要 `user.manage`）。 */
export function UsersPage() {
  const { message } = App.useApp();
  const users = useUsers();
  const unlockUser = useUnlockUser();
  const deleteUser = useDeleteUser();
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<User>();
  const [resetting, setResetting] = useState<User>();

  const showError = (err: unknown) => message.error(mutationErrorMessage(err));

  const columns: TableColumnsType<User> = [
    { title: '帳號', dataIndex: 'username' },
    { title: '顯示名稱', dataIndex: 'display_name' },
    { title: '角色', dataIndex: 'role_name' },
    {
      title: '狀態',
      key: 'status',
      render: (_, user) => (
        <Space size={4}>
          <StatusTag user={user} />
          {user.must_change_password && <Tag color="warning">待改密碼</Tag>}
        </Space>
      ),
    },
    {
      title: '最後登入時間',
      dataIndex: 'last_login_at',
      render: (value: string | null) =>
        value ? formatTaipei(value) : <Typography.Text type="secondary">從未登入</Typography.Text>,
    },
    {
      title: '操作',
      key: 'actions',
      render: (_, user) => (
        <Space size={0} wrap>
          <Button type="link" size="small" onClick={() => setEditing(user)}>
            編輯
          </Button>
          <Button type="link" size="small" onClick={() => setResetting(user)}>
            重設密碼
          </Button>
          {user.is_locked && (
            <Button
              type="link"
              size="small"
              onClick={() =>
                unlockUser.mutate(user, {
                  onSuccess: () => message.success(`已解除 ${user.username} 的鎖定`),
                  onError: showError,
                })
              }
            >
              解除鎖定
            </Button>
          )}
          <Popconfirm
            title={`確定要刪除帳號 ${user.username}？`}
            description="刪除後無法登入，操作紀錄會保留。"
            okText="刪除"
            okButtonProps={{ danger: true }}
            cancelText="取消"
            onConfirm={() =>
              deleteUser
                .mutateAsync(user)
                .then(() => void message.success(`已刪除 ${user.username}`), showError)
            }
          >
            <Button type="link" size="small" danger>
              刪除
            </Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <Card
      title="帳號管理"
      extra={
        <Space>
          <Button
            icon={<ReloadOutlined />}
            onClick={() => users.refetch()}
            loading={users.isFetching}
          >
            重新整理
          </Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreating(true)}>
            新增帳號
          </Button>
        </Space>
      }
    >
      {users.isError && (
        <Alert
          showIcon
          type="error"
          message={errorMessage(users.error)}
          style={{ marginBottom: 16 }}
        />
      )}
      <Table<User>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={users.data}
        loading={users.isPending}
        pagination={false}
      />
      <CreateUserModal open={creating} onClose={() => setCreating(false)} />
      <EditUserModal user={editing} onClose={() => setEditing(undefined)} />
      <ResetPasswordModal user={resetting} onClose={() => setResetting(undefined)} />
    </Card>
  );
}
