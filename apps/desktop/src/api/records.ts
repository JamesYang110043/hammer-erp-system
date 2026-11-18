import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { QueryKey } from '@tanstack/react-query';

import { LOOKUP_DEFAULT_LIMIT, commands } from '../bindings';
import type {
  CreateRecordRequest_Deserialize as CreateRecordRequest,
  FormulaPreviewRequest_Deserialize as FormulaPreviewRequest,
  CreateViewLayoutRequest_Deserialize as CreateViewLayoutRequest,
  Record as DataRecord,
  RecordListQuery_Deserialize as RecordListQuery,
  UpdateRecordRequest_Deserialize as UpdateRecordRequest,
  UpdateViewLayoutRequest_Deserialize as UpdateViewLayoutRequest,
  ViewLayout_Serialize as ViewLayout,
} from '../bindings';
import type { DocumentAction } from '../records/documents';
import { useDebouncedValue } from '../utils/useDebouncedValue';
import { auditLogsKey } from './auditLogs';
import { inventoryKey } from './inventory';
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

/** 每頁筆數的上限（伺服器 `RecordListQuery` 的上限）。 */
export const MAX_PAGE_SIZE = 200;

/**
 * 一個資料類別的全部未刪除資料（逐頁取得），給筆數不多、需要整份資料的選擇器使用
 * （例如商品分類的階層路徑）。`enabled` 為 `false` 時不查詢。
 *
 * 放在 {@link recordsKey} 底下：新增、修改、刪除資料後一起重新取得。
 */
export function useAllRecords(entityKey: string, enabled = true) {
  return useQuery({
    queryKey: [...recordsKey(entityKey), 'all'],
    queryFn: async () => {
      const items: DataRecord[] = [];
      for (let page = 1; ; page += 1) {
        const result = await unwrap(
          commands.listRecords(entityKey, {
            page,
            page_size: MAX_PAGE_SIZE,
            sort: null,
            filters: [],
          }),
        );
        items.push(...result.items);
        if (result.items.length === 0 || items.length >= result.total) {
          return items;
        }
      }
    },
    enabled,
  });
}

/**
 * 單筆資料（表單使用）。
 *
 * 不放在 {@link recordsKey} 底下：列表重新取得時不影響編輯中的表單。
 */
export const recordKey = (entityKey: string, id: string) => ['record', entityKey, id] as const;

export function useRecord(entityKey: string, id: string | undefined) {
  return useQuery({
    queryKey: recordKey(entityKey, id ?? ''),
    queryFn: () => unwrap(commands.getRecord(entityKey, id!)),
    enabled: id !== undefined,
    // 表單開啟期間不自動重新取得；有衝突時由使用者決定是否重新載入
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
}

/**
 * 關聯欄位的選擇器搜尋：`entityKey` 中標題符合 `q` 的未刪除資料（每個詞都要出現、不分大小寫），
 * 由新到舊，最多 {@link LOOKUP_DEFAULT_LIMIT} 筆。`q` 為空字串時回傳最新的資料。
 *
 * `enabled` 為 `false` 時不查詢（例如下拉選單還沒打開）。
 */
export function useLookupRecords(entityKey: string | undefined, q: string, enabled: boolean) {
  return useQuery({
    queryKey: ['lookup', entityKey ?? '', q] as const,
    queryFn: () =>
      unwrap(commands.lookupRecords(entityKey!, { q: q || null, limit: LOOKUP_DEFAULT_LIMIT })),
    enabled: enabled && entityKey !== undefined,
    // 輸入下一個字時保留上一次的結果，避免選單閃爍
    placeholderData: keepPreviousData,
  });
}

/** 表單停止輸入多久（毫秒）後更新公式預覽。 */
export const FORMULA_PREVIEW_DELAY_MS = 150;

/**
 * 公式欄位的即時預覽（Tauri command，使用 `crates/formula`，不需連線）。`req` 為 `undefined` 時不預覽。
 *
 * 輸入停止 {@link FORMULA_PREVIEW_DELAY_MS} 毫秒後計算；計算新的輸入時保留上一次的結果，避免閃爍。
 */
export function useFormulaPreview(req: FormulaPreviewRequest | undefined) {
  // 以 JSON 文字比較與延遲：表單每次重新繪製都會產生新的物件
  const json = req ? JSON.stringify(req) : '';
  const debounced = useDebouncedValue(json, FORMULA_PREVIEW_DELAY_MS);
  return useQuery({
    queryKey: ['formulaPreview', debounced] as const,
    queryFn: () => commands.previewFormulas(JSON.parse(debounced) as FormulaPreviewRequest),
    enabled: debounced !== '',
    staleTime: Infinity,
    gcTime: 60_000,
    placeholderData: keepPreviousData,
  });
}

/** 新增、修改資料成功後更新單筆快取，並重新取得列表、操作紀錄與 `extraKeys`。 */
function useRecordMutation<TVars>(
  entityKey: string,
  mutationFn: (vars: TVars) => Promise<DataRecord>,
  extraKeys: readonly QueryKey[] = [],
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (saved) => {
      queryClient.setQueryData(recordKey(entityKey, saved.id), saved);
      return Promise.all([
        queryClient.invalidateQueries({ queryKey: recordsKey(entityKey) }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
        ...extraKeys.map((queryKey) => queryClient.invalidateQueries({ queryKey })),
      ]);
    },
  });
}

export function useCreateRecord(entityKey: string) {
  return useRecordMutation(entityKey, (req: CreateRecordRequest) =>
    unwrap(commands.createRecord(entityKey, req)),
  );
}

export function useUpdateRecord(entityKey: string) {
  return useRecordMutation(entityKey, ({ id, req }: { id: string; req: UpdateRecordRequest }) =>
    unwrap(commands.updateRecord(entityKey, id, req)),
  );
}

/** 單據動作對應的 Tauri command。 */
const ACTION_COMMANDS = {
  post: commands.postRecord,
  void: commands.voidRecord,
  confirm: commands.confirmRecord,
  close: commands.closeRecord,
} as const;

/**
 * 系統單據的過帳、作廢，採購單的確認、結案（需要 `record.post`，伺服器檢查）。成功後更新單筆快取，
 * 並重新取得列表、操作紀錄與庫存查詢。
 *
 * 進貨單過帳、作廢會改變採購單的已進貨數量與狀態，所以一併重新取得其他單據的列表與單筆資料。
 */
export function useDocumentAction(entityKey: string, action: DocumentAction) {
  return useRecordMutation(
    entityKey,
    (record: Pick<DataRecord, 'id' | 'version'>) =>
      unwrap(ACTION_COMMANDS[action](entityKey, record.id, record.version)),
    // 過帳、作廢會異動庫存；進貨會回寫採購單
    [inventoryKey, ['records'], ['record']],
  );
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
