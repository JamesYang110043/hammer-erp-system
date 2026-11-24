import { ExclamationCircleOutlined } from '@ant-design/icons';
import { Checkbox, Space, Tag, Tooltip, Typography } from 'antd';

import { OptionColorTag } from '../../components/StatusTag';
import type { FieldDefinition, Record as DataRecord } from '../../bindings';
import {
  DELETED_SUFFIX,
  FORMULA_PENDING_TEXT,
  findOption,
  formatFieldText,
  formulaErrorLabel,
  isNumericType,
  relationLabel,
  valueField,
} from '../../records/display';
import { NumberText } from '../../components/NumberText';
import { AttachmentLink } from './AttachmentLink';
import { MISSING_ATTACHMENT_TEXT } from './FileInput';

function OptionTag({ field, value }: { field: FieldDefinition; value: string }) {
  const option = findOption(field, value);
  return <OptionColorTag color={option?.color}>{option?.label ?? value}</OptionColorTag>;
}

/**
 * 列表儲存格：依欄位型態顯示值。
 *
 * - 勾選：唯讀的勾選框
 * - 單選、多選：選項顏色標籤
 * - 關聯：關聯資料的標題（`record.relations`），已刪除時加上「（已刪除）」
 * - 附件：檔名（`record.attachments`），點擊下載
 * - 公式：依結果型態顯示；`record.formula_errors` 有該欄位時加上錯誤圖示（見 {@link FormulaValue}）
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
  if (field.data_type === 'formula') {
    return (
      <FormulaValue field={field} value={value} error={record?.formula_errors?.[field.field_key]} />
    );
  }
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
    default: {
      const text = formatFieldText(field, value) ?? '';
      // 數字、金額：等寬數字，負數為錯誤色
      return isNumericType(field) ? <NumberText>{text}</NumberText> : <>{text}</>;
    }
  }
}

/**
 * 公式欄位的值（唯讀）：依 `formula_result_type` 格式化。
 *
 * - `error`：計算錯誤的原因，顯示錯誤圖示，滑鼠移上去顯示原因（值為空值）
 * - `pending`：值要存檔後才會更新，加上「存檔後更新」標示
 */
export function FormulaValue({
  field,
  value,
  error,
  pending = false,
}: {
  field: FieldDefinition;
  value: unknown;
  error?: string | null;
  pending?: boolean;
}) {
  return (
    <Space size={4}>
      <FieldValue field={valueField(field)} value={value} />
      {error && (
        <Tooltip title={error}>
          <Typography.Text type="danger">
            <ExclamationCircleOutlined aria-label={formulaErrorLabel(error)} />
          </Typography.Text>
        </Tooltip>
      )}
      {pending && <Tag>{FORMULA_PENDING_TEXT}</Tag>}
    </Space>
  );
}
