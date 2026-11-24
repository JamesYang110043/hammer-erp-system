// antd 5 搭配 React 19 需要的相容層，必須最先載入
import '@ant-design/v5-patch-for-react-19';

import { QueryClientProvider } from '@tanstack/react-query';
import dayjs from 'dayjs';
import 'dayjs/locale/zh-tw';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';
import { createQueryClient } from './api/queryClient';
import { AppThemeProvider } from './components/AppThemeProvider';
import './styles.css';

dayjs.locale('zh-tw');

const queryClient = createQueryClient();

const root = document.getElementById('root');
if (!root) {
  throw new Error('找不到 #root 元素');
}

createRoot(root).render(
  <StrictMode>
    <AppThemeProvider>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </AppThemeProvider>
  </StrictMode>,
);
