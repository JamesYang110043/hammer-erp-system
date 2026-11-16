import { Checkbox, Space, Tag, Typography } from 'antd';

import type { FieldDefinition, Record as DataRecord } from '../../bindings';
import { DELETED_SUFFIX, findOption, formatFieldText, relationLabel } from '../../records/display';
import { AttachmentLink } from './AttachmentLink';
import { MISSING_ATTACHMENT_TEXT } from './FileInput';

function OptionTag({ field, value }: { field: FieldDefinition; value: string }) {
  const option = findOption(field, value);
  return <Tag color={option?.color ?? undefined}>{option?.label ?? value}</Tag>;
}

/**
 * 列表儲存格：依欄位型態顯示值。
 *
 * - 勾選：唯讀的勾選框
 * - 單選、多選：選項顏色標籤
 * - 關聯：關聯資料的標題（`record.relations`），已刪除時加上「（已刪除）」
 * - 附件：檔名（`record.attachments`），點擊下載
 * - 其他：見 {@link formatFieldText}（金額千分位、日期時間為台北時間）
 */
export function FieldValue({
  field,
  value,
  record,
}: {
  field: FieldDefinition;
  value: unknown;
  /** 值所屬的資料（關聯、附件的顯示資訊）。 */
  record?: DataRecord;
}) {
  if (value === null || value === undefined || value === '') {
    if (field.data_type === 'boolean') {
      return <Checkbox checked={false} disabled aria-label={field.label} />;
    }
    return <Typography.Text type="secondary">—</Typography.Text>;
  }
  switch (field.data_type) {
    case 'boolean':
      return <Checkbox checked={value === true} disabled aria-label={field.label} />;
    case 'select':
      return typeof value === 'string' ? <OptionTag field={field} value={value} /> : null;
    case 'multi_select':
      return Array.isArray(value) ? (
        <Space size={[0, 4]} wrap>
          {value.map((v) => (
            <OptionTag key={String(v)} field={field} value={String(v)} />
          ))}
        </Space>
      ) : null;
    case 'relation': {
      const ref = record?.relations?.[field.field_key];
      return ref ? (
        <Typography.Text type={ref.deleted ? 'secondary' : undefined}>
          {relationLabel(ref)}
        </Typography.Text>
      ) : (
        <Typography.Text type="secondary">{DELETED_SUFFIX}</Typography.Text>
      );
    }
    case 'file': {
      const attachment = record?.attachments?.[field.field_key];
      return attachment ? (
        <AttachmentLink attachment={attachment} />
      ) : (
        <Typography.Text type="secondary">{MISSING_ATTACHMENT_TEXT}</Typography.Text>
      );
    }
    default:
      return <>{formatFieldText(field, value)}</>;
  }
}
