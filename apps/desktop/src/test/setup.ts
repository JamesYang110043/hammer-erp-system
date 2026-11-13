// Vitest 全域設定：每個測試檔執行前載入（vite.config.ts 的 test.setupFiles）

// 與 main.tsx 相同：antd 5 搭配 React 19 的相容層
import '@ant-design/v5-patch-for-react-19';
import '@testing-library/jest-dom/vitest';

import { cleanup } from '@testing-library/react';
import { clearMocks } from '@tauri-apps/api/mocks';
import { afterEach } from 'vitest';

// antd 的 Grid、響應式元件會用到 matchMedia，jsdom 沒有實作
if (!window.matchMedia) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string): MediaQueryList => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

// dnd-kit 的鍵盤拖曳會呼叫 scrollIntoView，jsdom 沒有實作
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

afterEach(() => {
  cleanup();
  // 清除 mockIPC 設定，避免影響下一個測試
  clearMocks();
});
