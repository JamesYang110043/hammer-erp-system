import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { commands } from '../bindings';
import type {
  ChangeFieldTypeRequest,
  CreateEntityRequest,
  CreateFieldRequest,
  EntityDetail,
  FieldDefinition,
  UpdateFieldRequest,
} from '../bindings';
import { auditLogsKey } from './auditLogs';
import { unwrap } from './client';

/** 資料類別與欄位定義（寫入需要 `schema.manage`，伺服器檢查）。 */
export const entitiesKey = ['entities'] as const;
const entityKey = (key: string) => [...entitiesKey, key] as const;

export function useEntities({ enabled = true }: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: entitiesKey,
    queryFn: () => unwrap(commands.listEntities()),
    enabled,
  });
}

/** 資料類別與其所有欄位（含已停用），欄位依排序。 */
export function useEntity(key: string) {
  return useQuery({
    queryKey: entityKey(key),
    queryFn: () => unwrap(commands.getEntity(key)),
  });
}

/** 修改後重新取得資料類別與操作紀錄。 */
function useSchemaMutation<TVars, TData>(mutationFn: (vars: TVars) => Promise<TData>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: entitiesKey }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
      ]),
  });
}

export function useCreateEntity() {
  return useSchemaMutation((req: CreateEntityRequest) => unwrap(commands.createEntity(req)));
}

export function useCreateField(key: string) {
  return useSchemaMutation((req: CreateFieldRequest) => unwrap(commands.createField(key, req)));
}

export function useUpdateField(key: string) {
  return useSchemaMutation(({ fieldKey, req }: { fieldKey: string; req: UpdateFieldRequest }) =>
    unwrap(commands.updateField(key, fieldKey, req)),
  );
}

export function useDeactivateField(key: string) {
  return useSchemaMutation((field: FieldDefinition) =>
    unwrap(commands.deactivateField(key, field.field_key, field.version)),
  );
}

export function useChangeFieldType(key: string) {
  return useSchemaMutation(({ fieldKey, req }: { fieldKey: string; req: ChangeFieldTypeRequest }) =>
    unwrap(commands.changeFieldType(key, fieldKey, req)),
  );
}

/**
 * 調整欄位排序。`fields` 為新的順序（該資料類別的所有欄位）。
 *
 * 送出前先把畫面改成新順序（樂觀更新）；失敗時還原，成功時以伺服器回傳的欄位（新版本號）取代。
 */
export function useReorderFields(key: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (fields: FieldDefinition[]) =>
      unwrap(
        commands.reorderFields(key, {
          fields: fields.map((f) => ({ field_key: f.field_key, version: f.version })),
        }),
      ),
    onMutate: async (fields) => {
      await queryClient.cancelQueries({ queryKey: entityKey(key) });
      const previous = queryClient.getQueryData<EntityDetail>(entityKey(key));
      if (previous) {
        queryClient.setQueryData<EntityDetail>(entityKey(key), { ...previous, fields });
      }
      return { previous };
    },
    onError: (_err, _fields, context) => {
      if (context?.previous) {
        queryClient.setQueryData(entityKey(key), context.previous);
      }
    },
    onSuccess: (fields) => {
      const current = queryClient.getQueryData<EntityDetail>(entityKey(key));
      if (current) {
        queryClient.setQueryData<EntityDetail>(entityKey(key), { ...current, fields });
      }
    },
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: entityKey(key) }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
      ]),
  });
}
