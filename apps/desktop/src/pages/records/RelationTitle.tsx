import { Typography } from 'antd';

import { StatusTag } from '../../components/StatusTag';
import type { RelationRef } from '../../bindings';
import { relationLabel } from '../../records/display';

/** 關聯資料已停用時加上的標籤文字（§7.3 `inactive`）。 */
export const INACTIVE_TAG_TEXT = '已停用';

/**
 * 關聯資料的標題（§7.3）：已刪除時加上「（已刪除）」、灰字；已停用（`inactive`）時灰字並加上
 * `neutral` 的「已停用」標籤。預覽的關聯儲存格與關聯選擇器目前的值共用。
 */
export function RelationTitle({
  relation,
}: {
  relation: Pick<RelationRef, 'title' | 'deleted'> & { inactive?: boolean };
}) {
  const inactive = relation.inactive === true && !relation.deleted;
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, maxWidth: '100%' }}>
      <Typography.Text
        type={relation.deleted || inactive ? 'secondary' : undefined}
        ellipsis
        style={{ minWidth: 0 }}
      >
        {relationLabel(relation)}
      </Typography.Text>
      {inactive && (
        <StatusTag tone="neutral" style={{ marginInlineEnd: 0, flexShrink: 0 }}>
          {INACTIVE_TAG_TEXT}
        </StatusTag>
      )}
    </span>
  );
}
