import {
  DollarOutlined,
  FileTextOutlined,
  SettingOutlined,
  ShopOutlined,
  SwapOutlined,
  TeamOutlined,
} from '@ant-design/icons';
import type { ReactNode } from 'react';

export interface ModuleEntry {
  /** 路由路徑（不含開頭的 `/`） */
  path: string;
  label: string;
  icon: ReactNode;
}

/**
 * 左側選單的模組。目前都是空白頁，之後各里程碑逐一實作。
 *
 * 「系統設定」之後只有管理者看得到（M1 登入與權限完成後處理，權限仍以伺服器檢查為準）。
 */
export const MODULES: readonly ModuleEntry[] = [
  { path: 'inventory', label: '進出貨', icon: <SwapOutlined /> },
  { path: 'customers', label: '客戶', icon: <TeamOutlined /> },
  { path: 'suppliers', label: '廠商', icon: <ShopOutlined /> },
  { path: 'invoices', label: '發票', icon: <FileTextOutlined /> },
  { path: 'finance', label: '財務', icon: <DollarOutlined /> },
  { path: 'system', label: '系統設定', icon: <SettingOutlined /> },
];

/** 連線設定頁（本機設定，任何人都可以修改）。 */
export const CONNECTION_SETTINGS_PATH = 'connection';
