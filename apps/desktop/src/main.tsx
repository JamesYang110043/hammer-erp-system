// antd 5 搭配 React 19 需要的相容層，必須最先載入
import '@ant-design/v5-patch-for-react-19';

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { App as AntApp, ConfigProvider } from 'antd';
import zhTW from 'antd/locale/zh_TW';
import dayjs from 'dayjs';
import 'dayjs/locale/zh-tw';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { RouterProvider } from 'react-router';

import { router } from './router';
import './styles.css';

dayjs.locale('zh-tw');

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // 錯誤由各畫面自行顯示；連線失敗時不自動重試，避免畫面卡在載入中
      retry: false,
    },
  },
});

const root = document.getElementById('root');
if (!root) {
  throw new Error('找不到 #root 元素');
}

createRoot(root).render(
  <StrictMode>
    <ConfigProvider locale={zhTW}>
      <AntApp>
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </AntApp>
    </ConfigProvider>
  </StrictMode>,
);
