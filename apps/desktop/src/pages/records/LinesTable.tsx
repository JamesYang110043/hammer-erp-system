import { DeleteOutlined, PlusOutlined } from '@ant-design/icons';
import { Button, Form, Table, Typography } from 'antd';
import type { TableColumnsType } from 'antd';

import type { Attachment } from '../../api/attachments';
import type { RelationRef } from '../../bindings';
import { validateFieldValue } from '../../records/form';
import { LINES_FORM_KEY, LINE_ID_KEY, lineFormValues } from '../../records/lines';
import type { DetailTable } from '../../records/lines';
import { RecordFieldInput } from './RecordFieldInput';

/** 明細列目前的關聯與附件（顯示關聯標題、附件檔名用），鍵為明細 id。 */
export interface LineRefs {
  relations: Record<string, Record<string, RelationRef>>;
  attachments: Record<string, Record<string, Attachment>>;
}

interface Row {
  /** Form.List 的列鍵（不是明細 id）。 */
  key: number;
  /** 在 Form.List 中的位置。 */
  name: number;
}

/**
 * 一個明細子表的可編輯表格，放在主表的 antd `Form` 內（值在 `__lines.<明細子表代碼>`）。
 *
 * 欄位依明細子表的欄位定義產生（沿用 {@link RecordFieldInput} 與相同的驗證規則）；
 * 可以新增、刪除列，儲存格直接編輯。伺服器回傳的列錯誤由父元件以 `form.setFields` 標示在儲存格。
 */
export function LinesTable({
  table,
  canWrite,
  refs,
  onRowsChange,
}: {
  table: DetailTable;
  canWrite: boolean;
  refs: LineRefs;
  /** 新增、刪除列之後（列的變動不會觸發 Form 的 `onValuesChange`）。 */
  onRowsChange: () => void;
}) {
  const { entity, fields } = table;
  const form = Form.useFormInstance();
  const listName = [LINES_FORM_KEY, entity.key];

  return (
    <Form.List name={listName}>
      {(rows, { add, remove }) => {
        const lineId = (name: number) =>
          form.getFieldValue([...listName, name, LINE_ID_KEY]) as string | undefined;
        const columns: TableColumnsType<Row> = [
          {
            title: '#',
            key: '__index',
            width: 48,
            align: 'right',
            render: (_, row) => row.name + 1,
          },
          ...fields.map((field): TableColumnsType<Row>[number] => ({
            key: field.field_key,
            title:
              field.is_required && !field.is_readonly ? (
                <span>
                  <Typography.Text type="danger">* </Typography.Text>
                  {field.label}
                </span>
              ) : (
                field.label
              ),
            width: field.column_width ?? undefined,
            render: (_, row) => {
              const id = lineId(row.name);
              return (
                <Form.Item
                  name={[row.name, field.field_key]}
                  style={{ margin: 0 }}
                  rules={[
                    {
                      validator: (_r, value: unknown) => {
                        const error = validateFieldValue(field, value);
                        return error ? Promise.reject(new Error(error)) : Promise.resolve();
                      },
                    },
                  ]}
                >
                  <RecordFieldInput
                    field={field}
                    disabled={field.is_readonly || undefined}
                    relation={id ? refs.relations[id]?.[field.field_key] : undefined}
                    attachment={id ? refs.attachments[id]?.[field.field_key] : undefined}
                  />
                </Form.Item>
              );
            },
          })),
        ];
        if (canWrite) {
          columns.push({
            title: '操作',
            key: '__actions',
            width: 64,
            fixed: 'right',
            render: (_, row) => (
              <Button
                type="text"
                danger
                aria-label={`刪除第 ${row.name + 1} 列`}
                icon={<DeleteOutlined />}
                onClick={() => {
                  remove(row.name);
                  onRowsChange();
                }}
              />
            ),
          });
        }
        return (
          <section aria-label={entity.label} style={{ marginBottom: 24 }}>
            <Typography.Title level={5}>{entity.label}</Typography.Title>
            <Table<Row>
              size="small"
              bordered
              pagination={false}
              scroll={{ x: 'max-content' }}
              rowKey="key"
              columns={columns}
              dataSource={rows.map((r) => ({ key: r.key, name: r.name }))}
              locale={{ emptyText: '沒有明細' }}
            />
            {canWrite && (
              <Button
                type="dashed"
                icon={<PlusOutlined />}
                style={{ marginTop: 8 }}
                onClick={() => {
                  add(lineFormValues(fields));
                  onRowsChange();
                }}
              >
                新增{entity.label}
              </Button>
            )}
          </section>
        );
      }}
    </Form.List>
  );
}
