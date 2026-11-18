import { InboxOutlined } from '@ant-design/icons';

import { PERMISSIONS } from './bindings';
import type { ModuleEntry } from './modules';
import { ProductStockPage } from './pages/inventory/ProductStockPage';
import { StockBalancesPage } from './pages/inventory/StockBalancesPage';
import { STOCK_PATH } from './records/inventory';

/**
 * 左側選單「庫存」分組中，盤點調整單（`records/inventory` 的 `inventoryEntities`）之後的專屬頁面
 * （docs/architecture.md §8.10「庫存查詢 API」，需要 `record.read`）。
 *
 * 與 `modules.tsx` 分開：頁面會引用 `modules.tsx` 的路徑函式，放在一起會循環引用。
 */
export const INVENTORY_MODULES: readonly ModuleEntry[] = [
  {
    path: STOCK_PATH,
    label: '庫存查詢',
    icon: <InboxOutlined />,
    permission: PERMISSIONS.RECORD_READ,
    element: <StockBalancesPage />,
    children: [{ path: ':productId', element: <ProductStockPage /> }],
  },
];
