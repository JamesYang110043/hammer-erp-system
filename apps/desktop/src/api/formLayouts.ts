import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { commands } from '../bindings';
import type {
  FormLayout_Serialize as FormLayout,
  SaveFormLayoutRequest_Deserialize as SaveFormLayoutRequest,
} from '../bindings';
import { auditLogsKey } from './auditLogs';
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

/** 儲存或還原後，以回傳的版面更新快取，並重新取得操作紀錄。 */
function useFormLayoutMutation<TVars>(
  entityKey: string,
  mutationFn: (vars: TVars) => Promise<FormLayout>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (layout) => {
      queryClient.setQueryData(formLayoutKey(entityKey), layout);
      return queryClient.invalidateQueries({ queryKey: auditLogsKey });
    },
  });
}

/** 新增或修改表單版面（`version` 不符時為 `CONFLICT`）。 */
export function useSaveFormLayout(entityKey: string) {
  return useFormLayoutMutation(entityKey, (req: SaveFormLayoutRequest) =>
    unwrap(commands.saveFormLayout(entityKey, req)),
  );
}

/** 表單版面還原成預設，回傳自動產生的版面。 */
export function useResetFormLayout(entityKey: string) {
  return useFormLayoutMutation(entityKey, (version: number) =>
    unwrap(commands.resetFormLayout(entityKey, version)),
  );
}
