import { DeleteOutlined, PlusOutlined } from '@ant-design/icons';
import { Button, Form, Radio, Table, Typography } from 'antd';
import type { TableColumnsType } from 'antd';

import { SectionTitle } from '../../components/SectionTitle';
import type { Attachment } from '../../api/attachments';
import type { FieldDefinition, RelationRef } from '../../bindings';
import { isNumericType } from '../../records/display';
import { validateFieldValue } from '../../records/form';
import type { FormulaCell } from '../../records/formulaPreview';
import { LINES_FORM_KEY, LINE_ID_KEY, lineFormValues } from '../../records/lines';
import { LINE_FIELDS, STOCK_ADJUSTMENT_LINE } from '../../records/inventory';
import { isExclusiveLineField, rowsToClear } from '../../records/masterData';
import type { DetailTable } from '../../records/lines';
import {
  GOODS_RECEIPT_LINE,
  PURCHASE_RETURN_LINE,
  PURCHASING_FIELDS,
} from '../../records/purchasing';
import { FormulaValue } from './FieldValue';
import { ReceiptLotNoInput, ReturnableQty } from './PurchasingInputs';
import {
  SALES_FIELDS,
  SALES_RETURN_LINE,
  SHIPMENT_LINE,
  namedSalesLine,
} from '../../records/sales';
import { SalesProductSelect, ShipmentLotCell, ShipmentReturnableQty } from './SalesInputs';
import { RecordFieldInput } from './RecordFieldInput';
import { CurrentQty, LotNoInput, UnitCostInput } from './StockAdjustmentLineInputs';

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
 * 公式欄位唯讀，顯示父元件提供的預覽（`formulaCell`）。
 * 聯絡人的「主要聯絡人」以單選輸入：選了一列，其他列自動取消（{@link isExclusiveLineField}）。
 */
export function LinesTable({
  table,
  canWrite,
  refs,
  onRowsChange,
  formulaCell,
  showPending = false,
}: {
  table: DetailTable;
  canWrite: boolean;
  refs: LineRefs;
  /** 新增、刪除列之後（列的變動不會觸發 Form 的 `onValuesChange`）。 */
  onRowsChange: () => void;
  /** 公式欄位要顯示的值：明細在表單中的位置、既有明細的 id（新的明細為 `undefined`）。 */
  formulaCell?: (index: number, lineId: string | undefined, fieldKey: string) => FormulaCell;
  /** 是否標示「存檔後更新」（表單有未存檔的修改時）。 */
  showPending?: boolean;
}) {
  const { entity, fields } = table;
  // 不顯示的欄位（例如採購單帶入的採購明細）只放在表單值中，不列出
  const columnFields: FieldDefinition[] = [
    ...fields.filter((f) => f.is_visible),
    ...(table.formulas ?? []),
  ].sort((a, b) => a.sort_order - b.sort_order);
  const form = Form.useFormInstance();
  const listName = [LINES_FORM_KEY, entity.key];
  // 盤點調整明細：批號選擇、成本與目前數量（StockAdjustmentLineInputs）
  const isStockAdjustment = entity.key === STOCK_ADJUSTMENT_LINE;
  // 進貨明細：批號留空時過帳自動產生，過帳後連到庫存查詢
  const isGoodsReceipt = entity.key === GOODS_RECEIPT_LINE;
  // 進貨退回明細：批號（唯讀）連到庫存查詢；草稿顯示可退數量
  const isPurchaseReturn = entity.key === PURCHASE_RETURN_LINE;
  // 出貨明細：批號留空為先進先出；草稿顯示建議批號、可以指定批號
  const isShipment = entity.key === SHIPMENT_LINE;
  // 銷貨退回明細：草稿顯示可退數量；明細只能從出貨單帶入
  const isSalesReturn = entity.key === SALES_RETURN_LINE;
  // 訂單、出貨明細：商品可用客戶品名搜尋（表頭已選客戶時），選定後帶入品名
  const isNamedSales = namedSalesLine(entity.key);

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
          ...columnFields.map((field): TableColumnsType<Row>[number] =>
            field.data_type === 'formula'
              ? {
                  key: field.field_key,
                  title: field.label,
                  width: field.column_width ?? undefined,
                  align: isNumericType(field) ? 'right' : undefined,
                  render: (_, row) => {
                    const cell = formulaCell?.(row.name, lineId(row.name), field.field_key);
                    return (
                      <FormulaValue
                        field={field}
                        value={cell?.value}
                        error={cell?.error}
                        pending={showPending && (cell?.pending ?? false)}
                      />
                    );
                  },
                }
              : {
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
                    const rowPath = [...listName, row.name];
                    const disabled = field.is_readonly || undefined;
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
                        {(isGoodsReceipt || isPurchaseReturn || isShipment) &&
                        field.field_key === PURCHASING_FIELDS.lotNo ? (
                          <ReceiptLotNoInput
                            row={rowPath}
                            label={field.label}
                            disabled={disabled || !canWrite}
                            placeholder={isShipment ? '留空則先進先出' : undefined}
                          />
                        ) : isStockAdjustment && field.field_key === LINE_FIELDS.lotNo ? (
                          <LotNoInput
                            row={rowPath}
                            label={field.label}
                            disabled={disabled || !canWrite}
                          />
                        ) : isStockAdjustment && field.field_key === LINE_FIELDS.unitCost ? (
                          <UnitCostInput
                            row={rowPath}
                            field={field}
                            disabled={disabled || !canWrite}
                          />
                        ) : isNamedSales && field.field_key === SALES_FIELDS.product ? (
                          <SalesProductSelect
                            field={field}
                            aria-label={field.label}
                            disabled={field.is_readonly || undefined}
                            initial={id ? refs.relations[id]?.[field.field_key] : undefined}
                          />
                        ) : isExclusiveLineField(field) ? (
                          <ExclusiveRadio
                            label={`第 ${row.name + 1} 列${field.label}`}
                            disabled={field.is_readonly || undefined}
                            onSelect={() => {
                              const values = rows.map((r) =>
                                form.getFieldValue([...listName, r.name, field.field_key]),
                              );
                              for (const i of rowsToClear(values, row.name)) {
                                form.setFieldValue([...listName, i, field.field_key], false);
                              }
                            }}
                          />
                        ) : (
                          <RecordFieldInput
                            field={field}
                            disabled={field.is_readonly || undefined}
                            relation={id ? refs.relations[id]?.[field.field_key] : undefined}
                            attachment={id ? refs.attachments[id]?.[field.field_key] : undefined}
                          />
                        )}
                      </Form.Item>
                    );
                  },
                },
          ),
        ];
        if (isStockAdjustment && canWrite) {
          const lotIndex = columns.findIndex((c) => c.key === LINE_FIELDS.lotNo);
          columns.splice(lotIndex < 0 ? columns.length : lotIndex + 1, 0, {
            key: '__current_qty',
            title: '目前數量',
            align: 'right',
            render: (_, row) => <CurrentQty row={[...listName, row.name]} />,
          });
        }
        if (isPurchaseReturn && canWrite) {
          const qtyIndex = columns.findIndex((c) => c.key === PURCHASING_FIELDS.qty);
          columns.splice(qtyIndex < 0 ? columns.length : qtyIndex, 0, {
            key: '__returnable_qty',
            title: '可退數量',
            align: 'right',
            render: (_, row) => <ReturnableQty row={[...listName, row.name]} />,
          });
        }
        if (isShipment && canWrite) {
          const lotIndex = columns.findIndex((c) => c.key === PURCHASING_FIELDS.lotNo);
          columns.splice(lotIndex < 0 ? columns.length : lotIndex + 1, 0, {
            key: '__allocation',
            title: '建議批號',
            render: (_, row) => (
              <ShipmentLotCell listName={listName} row={row.name} onRowsChange={onRowsChange} />
            ),
          });
        }
        if (isSalesReturn && canWrite) {
          const qtyIndex = columns.findIndex((c) => c.key === PURCHASING_FIELDS.qty);
          columns.splice(qtyIndex < 0 ? columns.length : qtyIndex, 0, {
            key: '__returnable_qty',
            title: '可退數量',
            align: 'right',
            render: (_, row) => <ShipmentReturnableQty row={[...listName, row.name]} />,
          });
        }
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
            <SectionTitle>{entity.label}</SectionTitle>
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
            {/* 進貨退回、銷貨退回的明細只能從進貨單、出貨單帶入 */}
            {canWrite && !isPurchaseReturn && !isSalesReturn && (
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

/**
 * 整組明細只能有一列勾選的欄位（例如主要聯絡人）：單選按鈕，值為 `true` / `false`。
 *
 * 點選未選的列 → 選取並呼叫 `onSelect`（由父元件取消其他列）；再點一次已選的列 → 取消（可以沒有任何一列）。
 */
function ExclusiveRadio({
  value,
  onChange,
  onSelect,
  disabled,
  id,
  label,
}: {
  value?: unknown;
  onChange?: (value: boolean) => void;
  onSelect: () => void;
  disabled?: boolean;
  id?: string;
  label: string;
}) {
  const checked = value === true;
  return (
    <Radio
      id={id}
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onClick={() => {
        if (!checked) {
          onSelect();
        }
        onChange?.(!checked);
      }}
    />
  );
}
