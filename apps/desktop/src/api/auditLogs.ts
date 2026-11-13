import { keepPreviousData, useQuery } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { AuditLogQuery } from '../bindings';
import { unwrap } from './client';

/** 操作紀錄（需要 `audit.read`，伺服器檢查）。 */
export const auditLogsKey = ['auditLogs'] as const;

export function useAuditLogs(query: AuditLogQuery) {
  return useQuery({
    queryKey: [...auditLogsKey, query],
    queryFn: () => unwrap(commands.listAuditLogs(query)),
    // 換頁時保留上一頁資料，避免表格閃爍
    placeholderData: keepPreviousData,
  });
}
