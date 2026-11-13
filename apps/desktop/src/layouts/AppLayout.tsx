import {
  ApiOutlined,
  DownOutlined,
  KeyOutlined,
  LogoutOutlined,
  UserOutlined,
} from '@ant-design/icons';
import { App, Button, Dropdown, Layout, Menu, Modal, Space, Typography, theme } from 'antd';
import type { MenuProps } from 'antd';
import { useState } from 'react';
import { Outlet, useLocation, useNavigate } from 'react-router';

import { errorMessage } from '../api/client';
import { useEntities } from '../api/schema';
import { useCurrentUser, usePermissions } from '../auth/permission';
import { useLogout } from '../auth/session';
import { ChangePasswordForm } from '../components/ChangePasswordForm';
import { StatusBar } from '../components/StatusBar';
import { PERMISSIONS } from '../bindings';
import {
  CONNECTION_SETTINGS_PATH,
  ENTITIES_PATH,
  ENTITY_MENU_AFTER,
  MODULES,
  menuEntities,
} from '../modules';
import { entityIcon } from '../pages/schema/entityIcons';

const { Header, Sider, Content, Footer } = Layout;

type MenuItem = NonNullable<MenuProps['items']>[number];

/** 主畫面框架：左側模組選單、上方標題列（連線設定、使用者選單）、中間內容、底部狀態列。 */
export function AppLayout() {
  const navigate = useNavigate();
  const location = useLocation();
  const { token } = theme.useToken();
  const { message } = App.useApp();
  const user = useCurrentUser();
  const can = usePermissions();
  const logout = useLogout();
  const [changePasswordOpen, setChangePasswordOpen] = useState(false);

  // 資料類別選單需要 `record.read`；新增或修改資料類別後（invalidate `entities`）自動更新
  const canReadRecords = can(PERMISSIONS.RECORD_READ);
  const entities = useEntities({ enabled: canReadRecords });
  const entityItems: MenuItem[] = canReadRecords
    ? menuEntities(entities.data ?? []).map((e) => ({
        key: `${ENTITIES_PATH}/${e.key}`,
        icon: entityIcon(e.icon),
        label: e.label,
      }))
    : [];

  // 選單依權限顯示；頁面本身也有檢查（router.tsx 的 RequirePermission）
  const menuItems: MenuItem[] = MODULES.filter((m) => !m.permission || can(m.permission)).flatMap(
    (m) => {
      const item: MenuItem = { key: m.path, icon: m.icon, label: m.label };
      return m.path === ENTITY_MENU_AFTER ? [item, ...entityItems] : [item];
    },
  );

  // `/customers/123` → `customers`；`/entities/repair` → `entities/repair`
  const [first = '', second = ''] = location.pathname.split('/').slice(1);
  const selected = first === ENTITIES_PATH ? `${first}/${second}` : first;

  return (
    <Layout style={{ height: '100vh' }}>
      <Sider width={200} theme="light" style={{ borderRight: `1px solid ${token.colorSplit}` }}>
        <div style={{ padding: '16px 24px' }}>
          <Typography.Title level={4} style={{ margin: 0 }}>
            Hammer ERP
          </Typography.Title>
        </div>
        <Menu
          mode="inline"
          selectedKeys={[selected]}
          items={menuItems}
          onClick={({ key }) => navigate(`/${key}`)}
          style={{ borderInlineEnd: 'none' }}
        />
      </Sider>
      <Layout>
        <Header
          style={{
            background: token.colorBgContainer,
            borderBottom: `1px solid ${token.colorSplit}`,
            display: 'flex',
            justifyContent: 'flex-end',
            alignItems: 'center',
            paddingInline: 24,
          }}
        >
          <Space>
            <Button
              type={selected === CONNECTION_SETTINGS_PATH ? 'primary' : 'text'}
              icon={<ApiOutlined />}
              onClick={() => navigate(`/${CONNECTION_SETTINGS_PATH}`)}
            >
              連線設定
            </Button>
            <Dropdown
              trigger={['click']}
              menu={{
                items: [
                  { key: 'change-password', icon: <KeyOutlined />, label: '修改密碼' },
                  { type: 'divider' },
                  { key: 'logout', icon: <LogoutOutlined />, label: '登出' },
                ],
                onClick: ({ key }) => {
                  if (key === 'change-password') {
                    setChangePasswordOpen(true);
                  } else if (key === 'logout') {
                    logout.mutate(undefined, {
                      onError: (err) => message.error(errorMessage(err)),
                    });
                  }
                },
              }}
            >
              <Button type="text" icon={<UserOutlined />} loading={logout.isPending}>
                <Space size={4}>
                  {user?.display_name}
                  <DownOutlined />
                </Space>
              </Button>
            </Dropdown>
          </Space>
        </Header>
        <Content style={{ padding: 24, overflow: 'auto' }}>
          <Outlet />
        </Content>
        <Footer
          style={{
            padding: '4px 16px',
            background: token.colorBgContainer,
            borderTop: `1px solid ${token.colorSplit}`,
            fontSize: token.fontSizeSM,
          }}
        >
          <StatusBar />
        </Footer>
      </Layout>
      <Modal
        title="修改密碼"
        open={changePasswordOpen}
        onCancel={() => setChangePasswordOpen(false)}
        footer={null}
        destroyOnHidden
      >
        <ChangePasswordForm
          onSuccess={() => {
            setChangePasswordOpen(false);
            message.success('密碼已修改');
          }}
        />
      </Modal>
    </Layout>
  );
}
