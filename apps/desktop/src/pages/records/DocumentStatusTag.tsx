import { Tag } from 'antd';

import type { DocumentStatus } from '../../bindings';
import { DOCUMENT_STATUS_COLORS, DOCUMENT_STATUS_LABELS } from '../../records/documents';
import { postedLabel } from '../../records/invoicing';

/**
 * 單據狀態的顏色標籤：草稿（灰）、已過帳（綠）、已作廢（紅）。發票模組的已過帳顯示為「已開立」
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
    <Tag color={DOCUMENT_STATUS_COLORS[status]} style={{ marginInlineEnd: 0 }}>
      {label}
    </Tag>
  );
}
