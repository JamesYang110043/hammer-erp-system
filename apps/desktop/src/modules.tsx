import {
  AuditOutlined,
  DatabaseOutlined,
  DollarOutlined,
  FileTextOutlined,
  SettingOutlined,
  ShopOutlined,
  SwapOutlined,
  TeamOutlined,
  UserOutlined,
} from '@ant-design/icons';
import type { ReactNode } from 'react';

import type { Permission } from './auth/permission';
import { PERMISSIONS } from './bindings';
import { AuditLogsPage } from './pages/audit/AuditLogsPage';
import { EntitiesPage } from './pages/schema/EntitiesPage';
import { FieldsPage } from './pages/schema/FieldsPage';
import { UsersPage } from './pages/users/UsersPage';

export interface ModuleEntry {
  /** 路由路徑（不含開頭的 `/`） */
  path: string;
  label: string;
  icon: ReactNode;
  /** 需要的權限；沒有權限的使用者看不到選單，也無法開啟頁面。未指定時任何登入者都能使用。 */
  permission?: Permission;
  /** 頁面內容；未指定時顯示「尚未開放」。 */
  element?: ReactNode;
  /** 模組底下的頁面（例如 `:key` → `/schema/:key`），權限與模組相同。 */
  children?: { path: string; element: ReactNode }[];
}

/**
 * 左側選單的模組。沒有 `element` 的是尚未開發的空白頁，之後各里程碑逐一實作。
 *
 * 依 `permission` 顯示或隱藏只是輔助，權限仍以伺服器檢查為準（docs/architecture.md §6.2）。
 */
export const MODULES: readonly ModuleEntry[] = [
  { path: 'inventory', label: '進出貨', icon: <SwapOutlined /> },
  { path: 'customers', label: '客戶', icon: <TeamOutlined /> },
  { path: 'suppliers', label: '廠商', icon: <ShopOutlined /> },
  { path: 'invoices', label: '發票', icon: <FileTextOutlined /> },
  { path: 'finance', label: '財務', icon: <DollarOutlined /> },
  {
    path: 'users',
    label: '帳號管理',
    icon: <UserOutlined />,
    permission: PERMISSIONS.USER_MANAGE,
    element: <UsersPage />,
  },
  {
    path: 'schema',
    label: '資料類別',
    icon: <DatabaseOutlined />,
    permission: PERMISSIONS.SCHEMA_MANAGE,
    element: <EntitiesPage />,
    children: [{ path: ':key', element: <FieldsPage /> }],
  },
  {
    path: 'audit-logs',
    label: '操作紀錄',
    icon: <AuditOutlined />,
    permission: PERMISSIONS.AUDIT_READ,
    element: <AuditLogsPage />,
  },
  {
    path: 'system',
    label: '系統設定',
    icon: <SettingOutlined />,
    permission: PERMISSIONS.SETTINGS_MANAGE,
  },
];

/** 連線設定頁（本機設定，任何人都可以修改）。 */
export const CONNECTION_SETTINGS_PATH = 'connection';
