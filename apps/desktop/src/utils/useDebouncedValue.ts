import { useEffect, useState } from 'react';

/** `value` 停止變動 `delayMs` 毫秒後才更新的值（例如輸入時延遲查詢）。 */
export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return debounced;
}
