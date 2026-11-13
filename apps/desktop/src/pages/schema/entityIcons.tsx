import {
  AppstoreOutlined,
  CarOutlined,
  DatabaseOutlined,
  FileTextOutlined,
  FolderOutlined,
  HomeOutlined,
  ProfileOutlined,
  ShoppingOutlined,
  TagsOutlined,
  TeamOutlined,
  ToolOutlined,
} from '@ant-design/icons';
import type { ReactNode } from 'react';

/** 資料類別可選用的圖示。`entity_definitions.icon` 存放名稱（例如 `ToolOutlined`）。 */
export const ENTITY_ICONS: Record<string, ReactNode> = {
  AppstoreOutlined: <AppstoreOutlined />,
  DatabaseOutlined: <DatabaseOutlined />,
  FileTextOutlined: <FileTextOutlined />,
  FolderOutlined: <FolderOutlined />,
  ProfileOutlined: <ProfileOutlined />,
  ToolOutlined: <ToolOutlined />,
  TagsOutlined: <TagsOutlined />,
  TeamOutlined: <TeamOutlined />,
  ShoppingOutlined: <ShoppingOutlined />,
  CarOutlined: <CarOutlined />,
  HomeOutlined: <HomeOutlined />,
};

/** 圖示名稱 → 圖示；沒有設定或不認得時使用預設圖示。 */
export function entityIcon(name: string | null | undefined): ReactNode {
  return (name && ENTITY_ICONS[name]) || <AppstoreOutlined />;
}
