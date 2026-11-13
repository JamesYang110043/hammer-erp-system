import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { commands } from '../bindings';
import type {
  CreateViewLayoutRequest_Deserialize as CreateViewLayoutRequest,
  Record as DataRecord,
  RecordListQuery_Deserialize as RecordListQuery,
  UpdateViewLayoutRequest_Deserialize as UpdateViewLayoutRequest,
  ViewLayout_Serialize as ViewLayout,
} from '../bindings';
import { auditLogsKey } from './auditLogs';
import { unwrap } from './client';

/** 自訂資料類別的資料（讀取需要 `record.read`、寫入需要 `record.write`，伺服器檢查）。 */
export const recordsKey = (entityKey: string) => ['records', entityKey] as const;

/** 列表版面：全域版面與自己的版面，以及預設版面。 */
export const layoutsKey = (entityKey: string) => ['layouts', entityKey] as const;

export function useRecords(entityKey: string, query: RecordListQuery) {
  return useQuery({
    queryKey: [...recordsKey(entityKey), query],
    queryFn: () => unwrap(commands.listRecords(entityKey, query)),
    // 換頁、排序時保留上一頁資料，避免表格閃爍
    placeholderData: keepPreviousData,
  });
}

/** 刪除資料，成功後重新取得列表與操作紀錄。 */
export function useDeleteRecord(entityKey: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (record: Pick<DataRecord, 'id' | 'version'>) =>
      unwrap(commands.deleteRecord(entityKey, record.id, record.version)),
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: recordsKey(entityKey) }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
      ]),
  });
}

const layoutListKey = (entityKey: string) => [...layoutsKey(entityKey), 'list'] as const;

export function useLayouts(entityKey: string) {
  return useQuery({
    queryKey: layoutListKey(entityKey),
    queryFn: () => unwrap(commands.listLayouts(entityKey)),
  });
}

/** 開啟列表時要套用的版面：自己的預設 → 全域預設 → 依欄位定義產生。 */
export function useDefaultLayout(entityKey: string) {
  return useQuery({
    queryKey: [...layoutsKey(entityKey), 'default'],
    queryFn: () => unwrap(commands.getDefaultLayout(entityKey)),
  });
}

/**
 * 修改版面後重新取得版面列表、預設版面與操作紀錄。
 *
 * 新增、修改成功時先把回傳的版面寫進列表快取，畫面不必等重新取得就能顯示新的版本與設定。
 */
function useLayoutMutation<TVars, TData extends ViewLayout | null>(
  entityKey: string,
  mutationFn: (vars: TVars) => Promise<TData>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (saved) => {
      if (saved) {
        queryClient.setQueryData<ViewLayout[]>(layoutListKey(entityKey), (old) =>
          old ? [...old.filter((l) => l.id !== saved.id), saved] : old,
        );
      }
      return Promise.all([
        queryClient.invalidateQueries({ queryKey: layoutsKey(entityKey) }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
      ]);
    },
  });
}

export function useCreateLayout(entityKey: string) {
  return useLayoutMutation(entityKey, (req: CreateViewLayoutRequest) =>
    unwrap(commands.createLayout(entityKey, req)),
  );
}

export function useUpdateLayout(entityKey: string) {
  return useLayoutMutation(entityKey, ({ id, req }: { id: string; req: UpdateViewLayoutRequest }) =>
    unwrap(commands.updateLayout(entityKey, id, req)),
  );
}

export function useDeleteLayout(entityKey: string) {
  return useLayoutMutation(entityKey, (layout: Pick<ViewLayout, 'id' | 'version'>) =>
    unwrap(commands.deleteLayout(entityKey, layout.id, layout.version)),
  );
}
