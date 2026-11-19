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
import type { EntityDefinition } from '../bindings';
import {
  CONNECTION_SETTINGS_PATH,
  ENTITIES_PATH,
  ENTITY_MENU_AFTER,
  MASTER_DATA_MODULES,
  MODULES,
  masterDataEntities,
  menuEntityGroups,
} from '../modules';
import { INVENTORY_MODULES } from '../inventoryModules';
import { entityIcon } from '../pages/schema/entityIcons';
import { INVENTORY_GROUP_LABEL, inventoryEntities } from '../records/inventory';
import { PURCHASING_GROUP_LABEL, purchasingEntities } from '../records/purchasing';
import { SALES_GROUP_LABEL, salesEntities } from '../records/sales';
import { MASTER_DATA_LABEL } from '../schema/entities';

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
  const entityItem = (e: EntityDefinition): MenuItem => ({
    key: `${ENTITIES_PATH}/${e.key}`,
    icon: entityIcon(e.icon),
    label: e.label,
  });
  // 自訂資料類別依種類分組（主檔、單據），組內依選單排序
  const entityItems: MenuItem[] = canReadRecords
    ? menuEntityGroups(entities.data ?? []).map((g) => ({
        type: 'group',
        key: `${ENTITIES_PATH}-${g.kind}`,
        label: g.label,
        children: g.entities.map(entityItem),
      }))
    : [];
  // 系統主檔（商品、客戶、廠商…）與幣別、匯率放在選單最上方
  const masterData: MenuItem[] = [
    ...(canReadRecords ? masterDataEntities(entities.data ?? []) : []).map(entityItem),
    ...MASTER_DATA_MODULES.filter((m) => !m.permission || can(m.permission)).map((m): MenuItem => ({
      key: m.path,
      icon: m.icon,
      label: m.label,
    })),
  ];
  const masterDataItems: MenuItem[] =
    masterData.length > 0
      ? [
          {
            type: 'group',
            key: `${ENTITIES_PATH}-system`,
            label: MASTER_DATA_LABEL,
            children: masterData,
          },
          { type: 'divider', key: `${ENTITIES_PATH}-system-divider` },
        ]
      : [];

  // 進貨：採購單、進貨單（docs/architecture.md §8.8）
  const purchasing = (canReadRecords ? purchasingEntities(entities.data ?? []) : []).map(
    entityItem,
  );
  const purchasingItems: MenuItem[] =
    purchasing.length > 0
      ? [
          {
            type: 'group',
            key: 'purchasing-group',
            label: PURCHASING_GROUP_LABEL,
            children: purchasing,
          },
          { type: 'divider', key: 'purchasing-group-divider' },
        ]
      : [];

  // 銷貨：訂單、出貨單、銷貨退回（docs/architecture.md §8.9）
  const sales = (canReadRecords ? salesEntities(entities.data ?? []) : []).map(entityItem);
  const salesItems: MenuItem[] =
    sales.length > 0
      ? [
          { type: 'group', key: 'sales-group', label: SALES_GROUP_LABEL, children: sales },
          { type: 'divider', key: 'sales-group-divider' },
        ]
      : [];

  // 庫存：盤點調整單與庫存查詢（docs/architecture.md §8.10）
  const inventory: MenuItem[] = [
    ...(canReadRecords ? inventoryEntities(entities.data ?? []) : []).map(entityItem),
    ...INVENTORY_MODULES.filter((m) => !m.permission || can(m.permission)).map((m): MenuItem => ({
      key: m.path,
      icon: m.icon,
      label: m.label,
    })),
  ];
  const inventoryItems: MenuItem[] =
    inventory.length > 0
      ? [
          {
            type: 'group',
            key: 'inventory-group',
            label: INVENTORY_GROUP_LABEL,
            children: inventory,
          },
          { type: 'divider', key: 'inventory-group-divider' },
        ]
      : [];

  // 選單依權限顯示；頁面本身也有檢查（router.tsx 的 RequirePermission）
  const menuItems: MenuItem[] = [
    ...masterDataItems,
    ...purchasingItems,
    ...salesItems,
    ...inventoryItems,
    ...MODULES.filter((m) => !m.permission || can(m.permission)).flatMap((m) => {
      const item: MenuItem = { key: m.path, icon: m.icon, label: m.label };
      return m.path === ENTITY_MENU_AFTER ? [item, ...entityItems] : [item];
    }),
  ];

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
