import { Button, Result, Spin } from 'antd';
import { RouterProvider } from 'react-router';

import { errorMessage } from './api/client';
import { useSession } from './auth/session';
import { AuthLayout } from './layouts/AuthLayout';
import { ChangePasswordPage } from './pages/ChangePasswordPage';
import { LoginPage } from './pages/LoginPage';
import { SetupAdminPage } from './pages/SetupAdminPage';
import { router } from './router';

/**
 * 依登入狀態決定顯示哪個畫面（啟動流程見 `auth/session.ts`）。
 *
 * 只有已登入且不需要改密碼時才進入主畫面（路由）。
 */
export function App() {
  const session = useSession();

  if (session.isPending) {
    return (
      <div style={{ height: '100vh', display: 'grid', placeItems: 'center' }}>
        <Spin size="large" />
      </div>
    );
  }

  if (session.isError) {
    return (
      <AuthLayout title="無法連線">
        <Result
          status="warning"
          title="無法取得登入狀態"
          subTitle={errorMessage(session.error)}
          extra={
            <Button type="primary" onClick={() => session.refetch()} loading={session.isFetching}>
              重試
            </Button>
          }
        />
      </AuthLayout>
    );
  }

  const state = session.data;
  switch (state.kind) {
    case 'setup':
      return <SetupAdminPage />;
    case 'login':
      return <LoginPage notice={state.notice} />;
    case 'authenticated':
      return state.user.must_change_password ? (
        <ChangePasswordPage />
      ) : (
        <RouterProvider router={router} />
      );
  }
}
