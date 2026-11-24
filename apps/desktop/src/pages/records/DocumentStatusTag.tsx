import type { DocumentStatus } from '../../bindings';
import { StatusTag } from '../../components/StatusTag';
import { DOCUMENT_STATUS_LABELS } from '../../records/documents';
import { postedLabel } from '../../records/invoicing';
import { DOCUMENT_STATUS_TONES } from '../../status';

/**
 * 單據狀態標籤，顏色依語意狀態（src/status.ts 的 `DOCUMENT_STATUS_TONES`）：草稿（中性）、
 * 已過帳（成功）、已作廢（危險）…。發票模組的已過帳顯示為「已開立」
 * （進項憑證為「已登錄」，docs/architecture.md §8.11）。
 */
export function DocumentStatusTag({
  status,
  entityKey,
}: {
  status: DocumentStatus;
  entityKey?: string;
}) {
  const label =
    (status === 'posted' && entityKey ? postedLabel(entityKey) : undefined) ??
    DOCUMENT_STATUS_LABELS[status];
  return (
    <StatusTag tone={DOCUMENT_STATUS_TONES[status]} style={{ marginInlineEnd: 0 }}>
      {label}
    </StatusTag>
  );
}
