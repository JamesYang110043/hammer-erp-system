import { useQuery } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { FormLayout_Serialize as FormLayout } from '../bindings';
import { unwrap } from './client';

export type { FormLayout };

/** 表單版面（docs/architecture.md §7.7；讀取需要 `record.read`，修改需要 `schema.manage`）。 */
export const formLayoutKey = (entityKey: string) => ['form-layout', entityKey] as const;

/** 資料類別的表單版面；沒有設定時為伺服器依欄位定義自動產生的版面（`is_default`）。 */
export function useFormLayout(entityKey: string) {
  return useQuery({
    queryKey: formLayoutKey(entityKey),
    queryFn: () => unwrap(commands.getFormLayout(entityKey)),
  });
}
