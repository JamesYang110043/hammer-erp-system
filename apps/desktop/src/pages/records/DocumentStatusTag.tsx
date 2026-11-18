import { Tag } from 'antd';

import type { DocumentStatus } from '../../bindings';
import { DOCUMENT_STATUS_COLORS, DOCUMENT_STATUS_LABELS } from '../../records/documents';

/** 單據狀態的顏色標籤：草稿（灰）、已過帳（綠）、已作廢（紅）。 */
export function DocumentStatusTag({ status }: { status: DocumentStatus }) {
  return (
    <Tag color={DOCUMENT_STATUS_COLORS[status]} style={{ marginInlineEnd: 0 }}>
      {DOCUMENT_STATUS_LABELS[status]}
    </Tag>
  );
}
