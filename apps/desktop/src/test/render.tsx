import { QueryClientProvider } from '@tanstack/react-query';
import { render } from '@testing-library/react';
import type { RenderOptions } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';

import { createQueryClient } from '../api/queryClient';
import { AppThemeProvider } from '../components/AppThemeProvider';

/** 以與 main.tsx 相同的 Provider（AppThemeProvider：antd 繁中語系與主題、外觀；TanStack Query）渲染元件。 */
export function renderWithProviders(ui: ReactElement, options?: Omit<RenderOptions, 'wrapper'>) {
  // 每個測試用新的 QueryClient，避免快取互相影響；設定與 main.tsx 相同
  const queryClient = createQueryClient();
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <AppThemeProvider>
        <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
      </AppThemeProvider>
    );
  }
  return { queryClient, ...render(ui, { wrapper: Wrapper, ...options }) };
}
