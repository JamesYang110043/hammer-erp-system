import { Navigate, createHashRouter } from 'react-router';

import { RequirePermission } from './components/RequirePermission';
import { AppLayout } from './layouts/AppLayout';
import { PERMISSIONS } from './bindings';
import { CONNECTION_SETTINGS_PATH, ENTITIES_PATH, MODULES } from './modules';
import { ConnectionSettingsPage } from './pages/ConnectionSettingsPage';
import { PlaceholderPage } from './pages/PlaceholderPage';
import { RecordFormPage } from './pages/records/RecordFormPage';
import { RecordsPage } from './pages/records/RecordsPage';

// 只有登入後才會渲染（見 App.tsx）。
// 桌面程式沒有網址列，使用 hash 路由，重新整理時不需要伺服器端的路由設定
export const router = createHashRouter([
  {
    path: '/',
    element: <AppLayout />,
    children: [
      {
        index: true,
        element: <Navigate to={MODULES[0]?.path ?? CONNECTION_SETTINGS_PATH} replace />,
      },
      ...MODULES.flatMap((m) => [
        {
          path: m.path,
          element: (
            <RequirePermission permission={m.permission}>
              {m.element ?? <PlaceholderPage title={m.label} />}
            </RequirePermission>
          ),
        },
        ...(m.children ?? []).map((child) => ({
          path: `${m.path}/${child.path}`,
          element: <RequirePermission permission={m.permission}>{child.element}</RequirePermission>,
        })),
      ]),
      {
        path: `${ENTITIES_PATH}/:key`,
        element: (
          <RequirePermission permission={PERMISSIONS.RECORD_READ}>
            <RecordsPage />
          </RequirePermission>
        ),
      },
      {
        path: `${ENTITIES_PATH}/:key/new`,
        element: (
          <RequirePermission permission={PERMISSIONS.RECORD_WRITE}>
            <RecordFormPage />
          </RequirePermission>
        ),
      },
      {
        // 沒有 `record.write` 時只能檢視
        path: `${ENTITIES_PATH}/:key/:id`,
        element: (
          <RequirePermission permission={PERMISSIONS.RECORD_READ}>
            <RecordFormPage />
          </RequirePermission>
        ),
      },
      { path: CONNECTION_SETTINGS_PATH, element: <ConnectionSettingsPage /> },
      { path: '*', element: <Navigate to="/" replace /> },
    ],
  },
]);
