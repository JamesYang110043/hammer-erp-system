import {
  ArrowLeftOutlined,
  AuditOutlined,
  CheckCircleOutlined,
  CloseOutlined,
  FileDoneOutlined,
  ReloadOutlined,
  SaveOutlined,
  StopOutlined,
} from '@ant-design/icons';
import { Alert, App, Button, Form, Input, Space, Spin, Tooltip, Typography } from 'antd';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Navigate, useNavigate, useParams } from 'react-router';

import { StatusTag } from '../../components/StatusTag';
import { EmptyState } from '../../components/EmptyState';
import { Page } from '../../components/PageHeader';
import type { PageHeaderProps } from '../../components/PageHeader';
import { CommandError, apiErrorCode, errorMessage } from '../../api/client';
import {
  useCreateRecord,
  useDocumentAction,
  useFormulaPreview,
  useRecord,
  useUpdateRecord,
} from '../../api/records';
import { useFormLayout } from '../../api/formLayouts';
import { useEntities, useEntity, useEntityDetails } from '../../api/schema';
import { usePermission } from '../../auth/permission';
import { PERMISSIONS } from '../../bindings';
import type {
  EntityDefinition,
  EntityDetail,
  FieldDefinition,
  Record as DataRecord,
  StockInsufficientDetails_Serialize as StockInsufficientDetails,
} from '../../bindings';
import { newRecordPath, recordListPath, recordPath } from '../../modules';
import { useVoidInvoice } from '../../api/invoicing';
import {
  CUSTOMS,
  INVOICING_FIELDS,
  PURCHASE_ALLOWANCE,
  PURCHASE_INVOICE,
  SALES_ALLOWANCE,
  SALES_INVOICE,
  invoicingActionLabel,
  invoicingConfirmText,
  invoicingStatusNote,
  isInvoicingDocument,
} from '../../records/invoicing';
import {
  InvoiceLinksNote,
  PurchaseAllowanceAutofill,
  PurchaseInvoiceAutofill,
  ReceiptPicker,
  SalesAllowanceAutofill,
  SalesInvoiceAutofill,
  ShipmentPicker,
  TaxPeriodNote,
  VoidInvoiceModal,
} from './InvoicingInputs';
import type { VoidDetails } from './InvoicingInputs';
import {
  DOCUMENT_ACTION_LABELS,
  actionConfirmText,
  availableActions,
  documentActionError,
  documentStatus,
  documentStatusNote,
  isEditableDocument,
  isSystemDocument,
} from '../../records/documents';
import { withFormDefaults } from '../../records/inventory';
import type { DocumentAction, DocumentActionError } from '../../records/documents';
import {
  baselineData,
  buildCreateData,
  buildUpdateData,
  changedFieldKeys,
  formFields,
  initialFormValues,
  recordFieldErrors,
  validateFieldValue,
} from '../../records/form';
import type { RecordFormValues } from '../../records/form';
import {
  FORM_BLOCKS,
  generatedFormLayout,
  linesBlockDetail,
  resolveFormLayout,
} from '../../records/formLayout';
import type { FormLayoutConfig } from '../../records/formLayout';
import { buildPreviewRequest, formulaCell, formulaFields } from '../../records/formulaPreview';
import type { PreviewTable } from '../../records/formulaPreview';
import {
  LINES_FORM_KEY,
  buildCreateLines,
  buildUpdateLines,
  detailEntitiesOf,
  initialLinesValues,
  lineErrorPath,
  lineErrorText,
  linesBaseline,
  linesChanged,
  recordLineErrors,
  showsRecordNo,
} from '../../records/lines';
import type { DetailTable, LineFormValues } from '../../records/lines';
import {
  GOODS_RECEIPT,
  PURCHASE_RETURN,
  PURCHASING_FIELDS,
  lineFormFields,
  pricedDocument,
} from '../../records/purchasing';
import { entityIcon } from '../schema/entityIcons';
import { formatTaipei, taipeiToday } from '../../utils/time';
import { DocumentStatusTag } from './DocumentStatusTag';
import { FormulaValue } from './FieldValue';
import { FormLayoutRenderer } from './FormLayoutRenderer';
import { LinesTable } from './LinesTable';
import type { LineRefs } from './LinesTable';
import { ExchangeRateInput, PurchaseReturnAutofill, PurchasingAutofill } from './PurchasingInputs';
import { useAmountPreview } from './useAmountPreview';
import { SalesAutofill, SalesReturnAutofill, UsedLotsTable } from './SalesInputs';
import { ShipmentAllocationContext, useShipmentAllocation } from './shipmentAllocation';
import { unshippedByLine, useSalesOrder } from '../../api/sales';
import {
  SALES_FIELDS,
  SALES_RETURN,
  SHIPMENT,
  SHIPMENT_LINE,
  manualLotShortages,
  overShippedRows,
  salesLineKey,
} from '../../records/sales';
import { formatQty } from '../../records/documents';
import { RecordFieldInput } from './RecordFieldInput';
import { splitViewPath, usesSplitView } from '../../records/masterSplit';
import { useUnsavedGuard } from './useUnsavedGuard';

/** 樂觀鎖定衝突（409）的提示（docs/architecture.md §9.4）。 */
export const RECORD_CONFLICT_MESSAGE = '資料已被其他人修改';

/** 新增資料時單號欄位的提示（單號由伺服器在存檔時產生）。 */
export const RECORD_NO_PENDING = '存檔後產生';

/** 單據動作按鈕的圖示。 */
const ACTION_ICONS: Record<DocumentAction, ReactNode> = {
  post: <CheckCircleOutlined />,
  confirm: <CheckCircleOutlined />,
  close: <FileDoneOutlined />,
  void: <StopOutlined />,
};

/** 表單有未存檔的修改時，過帳、作廢按鈕的提示。 */
export const SAVE_BEFORE_ACTION = '請先儲存目前的修改';

type FormValues = RecordFormValues & { [LINES_FORM_KEY]?: Record<string, LineFormValues[]> };

/** 過帳時庫存不足（`STOCK_INSUFFICIENT`）：每一列不足的明細在數量欄位標示錯誤。 */
function shortageCells(err: unknown): { name: (string | number)[]; errors: string[] }[] {
  if (apiErrorCode(err) !== 'STOCK_INSUFFICIENT' || !(err instanceof CommandError)) {
    return [];
  }
  const details = err.error.kind === 'api' ? err.error.details : undefined;
  const items = (details as Partial<StockInsufficientDetails> | undefined)?.items ?? [];
  return items.flatMap((item) =>
    item.lines.map((line) => ({
      name: [LINES_FORM_KEY, line.detail_key, line.index, 'qty'],
      errors: [`庫存不足，還差 ${formatQty(item.shortage)}`],
    })),
  );
}

/** 明細列目前的關聯與附件，鍵為明細 id。 */
function lineRefs(record?: DataRecord): LineRefs {
  const refs: LineRefs = { relations: {}, attachments: {} };
  for (const lines of Object.values(record?.lines ?? {})) {
    for (const line of lines) {
      refs.relations[line.id] = line.relations ?? {};
      refs.attachments[line.id] = line.attachments ?? {};
    }
  }
  return refs;
}

/** 明細列的既有資料（公式欄位顯示上一次存檔的值用），鍵為明細 id。 */
function savedLines(record?: DataRecord): Map<string, DataRecord> {
  return new Map(
    Object.values(record?.lines ?? {})
      .flat()
      .map((line) => [line.id, line]),
  );
}

/**
 * 新增或編輯一筆資料的表單。`record` 為 `undefined` 時是新增。
 *
 * 公式欄位唯讀：輸入其他欄位或明細時，以 Tauri command 即時預覽（`records/formulaPreview`）；
 * 預覽有錯誤時顯示錯誤提示，但不阻擋存檔（存檔時以伺服器計算為準，§7.6）。
 *
 * 父元件在資料重新載入（存檔成功、衝突後重新載入）時以 `key` 重設本元件。
 */
function RecordForm({
  entity,
  fields,
  details,
  detailDefs,
  layout,
  record,
  onReload,
  panel,
  onDirtyChange,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  /** 明細子表（欄位已依表單規則篩選）。 */
  details: DetailTable[];
  /** 明細子表完整的欄位定義（公式預覽用）。 */
  detailDefs: EntityDetail[];
  /** 表單版面（§7.7）；讀取失敗時為 `undefined`，依欄位定義產生。 */
  layout?: FormLayoutConfig;
  record?: DataRecord;
  onReload: () => void;
  /** 放在分割檢視右側時，存檔與取消由外部處理（見 {@link RecordFormPanel}）。 */
  panel?: RecordFormPanelActions;
  /** 未存檔狀態改變時（同步呼叫；離開提醒由外部的 {@link useUnsavedGuard} 處理）。 */
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const entityKey = entity.key;
  const navigate = useNavigate();
  const { message, modal } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const canPost = usePermission(PERMISSIONS.RECORD_POST);
  // 系統單據：只有草稿可以修改；過帳、作廢依狀態顯示（伺服器同樣檢查）
  const isDocument = isSystemDocument(entity);
  const status = isDocument ? documentStatus(record) : undefined;
  const editable = canWrite && isEditableDocument(entity, record);
  const actions = record && canPost ? availableActions(entityKey, status) : [];
  const mutations: Record<DocumentAction, ReturnType<typeof useDocumentAction>> = {
    post: useDocumentAction(entityKey, 'post'),
    void: useDocumentAction(entityKey, 'void'),
    confirm: useDocumentAction(entityKey, 'confirm'),
    close: useDocumentAction(entityKey, 'close'),
  };
  // 發票模組的作廢帶日期與原因（§8.11「開立與作廢」）
  const invoicing = isInvoicingDocument(entityKey);
  const voidInvoice = useVoidInvoice(entityKey);
  const [voidOpen, setVoidOpen] = useState(false);
  const actionLabel = (action: DocumentAction) =>
    invoicingActionLabel(entityKey, action) ?? DOCUMENT_ACTION_LABELS[action];
  const actionPending = Object.values(mutations).some((m) => m.isPending) || voidInvoice.isPending;
  // 已開立的發票（憑證）可以開立折讓；海關繳納證不可折讓（§8.11）
  const allowanceEntity =
    entityKey === SALES_INVOICE
      ? SALES_ALLOWANCE
      : entityKey === PURCHASE_INVOICE && record?.data[INVOICING_FIELDS.docType] !== CUSTOMS
        ? PURCHASE_ALLOWANCE
        : undefined;
  // 採購單、進貨單：幣別、匯率帶入與金額預覽（§8.8）
  const priced = pricedDocument(entityKey) !== undefined;
  const [actionError, setActionError] = useState<DocumentActionError>();
  const createRecord = useCreateRecord(entityKey);
  const updateRecord = useUpdateRecord(entityKey);
  const saving = createRecord.isPending || updateRecord.isPending;

  const visible = useMemo(() => formFields(fields), [fields]);
  // 表單上的欄位與公式欄位，依欄位排序
  const displayed = useMemo(
    () => [...visible, ...formulaFields(fields)].sort((a, b) => a.sort_order - b.sort_order),
    [visible, fields],
  );
  // 表單版面：伺服器的版面再依畫面上能顯示的欄位與明細表整理（不會遺漏欄位或系統區塊）
  const formLayout = useMemo(() => {
    const content = {
      entityKey,
      fieldKeys: displayed.map((f) => f.field_key),
      detailKeys: details.map((d) => d.entity.key),
    };
    return resolveFormLayout(layout ?? generatedFormLayout(content), content);
  }, [layout, entityKey, displayed, details]);
  // 有錯誤的欄位：所在的收折區塊自動展開
  const [revealFields, setRevealFields] = useState<string[]>();
  const initialValues = useMemo<FormValues>(
    () => ({
      ...initialFormValues(visible, record?.data),
      [LINES_FORM_KEY]: initialLinesValues(details, record),
    }),
    [visible, details, record],
  );
  const baseline = useMemo(() => baselineData(visible, record?.data), [visible, record]);
  const lineBaseline = useMemo(() => linesBaseline(details, record), [details, record]);
  const refs = useMemo(() => lineRefs(record), [record]);
  const savedLineById = useMemo(() => savedLines(record), [record]);

  // 公式預覽：表單目前的值（onValuesChange 時更新）
  const [current, setCurrent] = useState<FormValues>(initialValues);
  const previewTables = useMemo(() => {
    const shown = new Map(details.map((d) => [d.entity.key, d.fields]));
    return {
      master: { fields, shown: visible } satisfies PreviewTable,
      details: new Map(
        detailDefs.map((d) => [
          d.entity.key,
          { fields: d.fields, shown: shown.get(d.entity.key) ?? [] } satisfies PreviewTable,
        ]),
      ),
    };
  }, [fields, visible, details, detailDefs]);
  const previewRequest = useMemo(
    () =>
      buildPreviewRequest(
        previewTables.master,
        previewTables.details,
        current,
        current[LINES_FORM_KEY],
        record,
      ),
    [previewTables, current, record],
  );
  const preview = useFormulaPreview(previewRequest).data;
  useAmountPreview(form, entityKey, current, editable);
  // 訂單、出貨單（§8.9）：售價、訂單帶入；出貨單的建議批號與存檔前的數量檢查
  // 銷貨退回的稅別、單價等由出貨單帶入，不使用 SalesAutofill
  const isSales = salesLineKey(entityKey) !== undefined && entityKey !== SALES_RETURN;
  const isShipment = entityKey === SHIPMENT;
  const allocation = useShipmentAllocation(current, editable && isShipment);
  const shipmentOrderId = current[SALES_FIELDS.salesOrder];
  const shipmentOrder = useSalesOrder(
    editable && isShipment && typeof shipmentOrderId === 'string' && shipmentOrderId !== ''
      ? shipmentOrderId
      : undefined,
  );

  const [dirty, setDirty] = useState(false);
  const [conflict, setConflict] = useState(false);
  // 離開提醒在導覽當下讀取；存檔成功後要立刻導覽，所以同步通知外部，不能等 state 更新
  const dirtyChangeRef = useRef(onDirtyChange);
  useEffect(() => {
    dirtyChangeRef.current = onDirtyChange;
  }, [onDirtyChange]);
  const markDirty = (value: boolean) => {
    setDirty(value);
    dirtyChangeRef.current?.(value);
  };
  // 表單重設（重新載入、換一筆）或關閉時，外部的未存檔狀態也清除
  useEffect(() => () => dirtyChangeRef.current?.(false), []);

  const onValuesChange = () => {
    const values = form.getFieldsValue(true) as FormValues;
    setCurrent(values);
    markDirty(
      changedFieldKeys(visible, values, baseline).length > 0 ||
        linesChanged(details, values[LINES_FORM_KEY], lineBaseline),
    );
  };

  const onSaveError = (err: unknown) => {
    if (apiErrorCode(err) === 'CONFLICT') {
      setConflict(true);
      return;
    }
    const fieldErrors = recordFieldErrors(err) ?? [];
    const lineErrors = recordLineErrors(err) ?? [];
    if (fieldErrors.length > 0 || lineErrors.length > 0) {
      const shown = new Set(visible.map((f) => f.field_key));
      const lines = (form.getFieldValue(LINES_FORM_KEY) ?? {}) as Record<string, LineFormValues[]>;
      const rowCounts = Object.fromEntries(
        Object.entries(lines).map(([key, rows]) => [key, rows.length]),
      );
      const cells: { name: (string | number)[]; errors: string[] }[] = [];
      const others: string[] = [];
      for (const e of fieldErrors) {
        if (shown.has(e.field_key)) {
          cells.push({ name: [e.field_key], errors: [e.message] });
        } else {
          others.push(`${e.field_key}：${e.message}`);
        }
      }
      for (const e of lineErrors) {
        const path = lineErrorPath(e, details, rowCounts);
        if (path) {
          cells.push({ name: path, errors: [e.message] });
        } else {
          others.push(lineErrorText(e, details));
        }
      }
      form.setFields(cells);
      setRevealFields(cells.map((c) => String(c.name[0])));
      message.error(others.length > 0 ? others.join('；') : '資料有誤，請檢查標示的欄位');
      return;
    }
    message.error(errorMessage(err));
  };

  const onActionError = (target: DocumentAction) => (err: unknown) => {
    // 版本衝突沿用存檔衝突的提示（重新載入）；其他錯誤以易懂的訊息列在表單上方
    if (apiErrorCode(err) === 'CONFLICT') {
      setConflict(true);
      return;
    }
    const labels = Object.fromEntries(fields.map((f) => [f.field_key, f.label]));
    setActionError(documentActionError(err, target, labels, actionLabel(target)));
  };

  const runVoidInvoice = (details: VoidDetails) => {
    if (!record) {
      return;
    }
    voidInvoice
      .mutateAsync({ record, voidDate: details.voidDate, voidReason: details.voidReason })
      .then(
        () => {
          setVoidOpen(false);
          message.success('已作廢');
          onReload();
        },
        (err: unknown) => {
          setVoidOpen(false);
          onActionError('void')(err);
        },
      );
  };

  const runAction = (target: DocumentAction) => {
    if (!record) {
      return;
    }
    if (invoicing && target === 'void') {
      setVoidOpen(true);
      return;
    }
    const mutation = mutations[target];
    const label = actionLabel(target);
    modal.confirm({
      title: `確定要${label}這張${entity.label}？`,
      content: invoicingConfirmText(entityKey, target) ?? actionConfirmText(entityKey, target),
      okText: label,
      okButtonProps: target === 'void' ? { danger: true } : undefined,
      cancelText: '取消',
      onOk: () =>
        mutation.mutateAsync(record).then(
          () => void message.success(`已${label}`),
          (err: unknown) => {
            // 版本衝突沿用存檔衝突的提示（重新載入）；其他錯誤以易懂的訊息列在表單上方
            if (apiErrorCode(err) === 'CONFLICT') {
              setConflict(true);
            } else {
              const labels = Object.fromEntries(fields.map((f) => [f.field_key, f.label]));
              setActionError(documentActionError(err, target, labels, actionLabel(target)));
              // 欄位錯誤（例如確認時沒有匯率）同時標示在欄位上
              const fieldErrors = (recordFieldErrors(err) ?? []).filter((e) =>
                visible.some((f) => f.field_key === e.field_key),
              );
              form.setFields([
                ...fieldErrors.map((e) => ({ name: [e.field_key], errors: [e.message] })),
                // 庫存不足：標示是哪幾列（數量欄位）
                ...shortageCells(err),
              ]);
            }
          },
        ),
    });
  };

  /**
   * 出貨單存檔前的檢查（§8.9）：超過訂單的未出貨數量、找不到批號 → 標示在儲存格並停止；
   * 手動指定批號的數量超過可用數量 → 標示並詢問是否仍要存檔（草稿不檢查庫存，過帳時才檢查）。
   */
  const checkShipment = (values: FormValues): boolean => {
    const rows = values[LINES_FORM_KEY]?.[SHIPMENT_LINE] ?? [];
    const cell = (i: number, key: string, text: string) => ({
      name: [LINES_FORM_KEY, SHIPMENT_LINE, i, key],
      errors: [text],
    });
    const blocking = [
      ...[
        ...overShippedRows(
          rows.map((r) => ({ salesOrderLineId: r[SALES_FIELDS.salesOrderLine], qty: r.qty })),
          unshippedByLine(shipmentOrder.data),
        ),
      ].map(([i, limit]) => cell(i, SALES_FIELDS.qty, `超過未出貨數量 ${formatQty(limit)}`)),
      ...allocation.errors.map((e) => cell(e.row, e.field_key ?? SALES_FIELDS.lotNo, e.message)),
    ];
    if (blocking.length > 0) {
      form.setFields(blocking);
      message.error('明細的數量或批號有誤，請檢查標示的欄位');
      return false;
    }
    const lotNos = new Map(
      rows.map((r, i) => [i, typeof r[SALES_FIELDS.lotNo] === 'string' ? String(r.lot_no) : '']),
    );
    const shortages = allocation.current
      ? manualLotShortages(allocation.allocations, lotNos)
      : new Map<number, string>();
    if (shortages.size === 0) {
      return true;
    }
    form.setFields([...shortages].map(([i, text]) => cell(i, SALES_FIELDS.lotNo, text)));
    modal.confirm({
      title: '手動指定的批號數量不足',
      content: (
        <ul style={{ margin: 0, paddingInlineStart: 20 }}>
          {[...shortages].map(([i, text]) => (
            <li key={i}>
              第 {i + 1} 列：{text}
            </li>
          ))}
        </ul>
      ),
      okText: '仍要存檔',
      cancelText: '返回修改',
      onOk: () => save(values),
    });
    return false;
  };

  const onFinish = () => {
    // 包含 Form.List 內的明細列（onFinish 的參數只有註冊過的欄位，明細的 __id 不在其中）
    const values = form.getFieldsValue(true) as FormValues;
    if (isShipment && !checkShipment(values)) {
      return;
    }
    save(values);
  };

  const save = (values: FormValues) => {
    const lineValues = values[LINES_FORM_KEY];
    if (!record) {
      const lines = buildCreateLines(details, lineValues);
      createRecord.mutate(
        { data: buildCreateData(visible, values), ...(lines ? { lines } : {}) },
        {
          onSuccess: (saved) => {
            markDirty(false);
            message.success('已新增');
            if (panel) {
              panel.onSaved(saved);
            } else {
              navigate(recordPath(entityKey, saved.id), { replace: true });
            }
          },
          onError: onSaveError,
        },
      );
      return;
    }
    const data = buildUpdateData(visible, values, baseline);
    const lines = buildUpdateLines(details, lineValues, lineBaseline);
    if (Object.keys(data).length === 0 && !lines) {
      message.info('沒有需要儲存的修改');
      return;
    }
    // 存檔成功時快取更新，父元件會以新的 key 重設本元件；`mutate` 的回呼在元件卸載後不會執行，
    // 所以用 Promise 確保存檔後的處理（回到預覽）一定會執行
    updateRecord
      .mutateAsync({
        id: record.id,
        req: { version: record.version, data, ...(lines ? { lines } : {}) },
      })
      .then((saved) => {
        markDirty(false);
        message.success('已儲存');
        panel?.onSaved(saved);
      }, onSaveError);
  };

  const displayedByKey = new Map(displayed.map((f) => [f.field_key, f]));

  /** 表單上的一個欄位：公式欄位唯讀顯示預覽值，其他欄位為輸入元件。 */
  const renderFieldItem = (field: FieldDefinition): ReactNode => {
    if (field.data_type === 'formula') {
      const cell = formulaCell(preview, null, 0, field.field_key, {
        data: record?.data,
        errors: record?.formula_errors,
      });
      return (
        <Form.Item label={field.label} extra={field.help_text ?? undefined}>
          <FormulaValue
            field={field}
            value={cell.value}
            error={cell.error}
            pending={cell.pending && (dirty || !record)}
          />
        </Form.Item>
      );
    }
    return (
      <Form.Item
        name={field.field_key}
        label={field.label}
        extra={field.help_text ?? undefined}
        required={field.is_required && !field.is_readonly}
        rules={[
          {
            validator: (_, value: unknown) => {
              const error = validateFieldValue(field, value);
              return error ? Promise.reject(new Error(error)) : Promise.resolve();
            },
          },
        ]}
      >
        {/* 傳 false 會蓋過 Form 的 disabled（唯讀者），只在唯讀欄位時指定 */}
        {priced && field.field_key === PURCHASING_FIELDS.rate ? (
          <ExchangeRateInput
            entityKey={entityKey}
            field={field}
            disabled={!editable || field.is_readonly || undefined}
          />
        ) : (
          <RecordFieldInput
            field={field}
            // 進貨退回建立後不可更換進貨單（§8.8 `purchase_returns`）
            disabled={
              field.is_readonly ||
              (entityKey === PURCHASE_RETURN &&
                record !== undefined &&
                field.field_key === PURCHASING_FIELDS.goodsReceipt) ||
              // 銷貨退回建立後不可更換出貨單（§8.9 `sales_returns`）
              (entityKey === SALES_RETURN &&
                record !== undefined &&
                field.field_key === SALES_FIELDS.shipment) ||
              undefined
            }
            relation={record?.relations?.[field.field_key]}
            attachment={record?.attachments?.[field.field_key]}
          />
        )}
      </Form.Item>
    );
  };

  /**
   * 系統區塊（§7.7）：位置由版面決定，是否顯示由區塊本身判斷（例如只在可以編輯時）。
   * 回傳 `null` 時不顯示。
   */
  const renderBlock = (code: string): ReactNode => {
    const detailKey = linesBlockDetail(code);
    if (detailKey !== undefined) {
      const table = details.find((d) => d.entity.key === detailKey);
      return table ? (
        <LinesTable
          table={table}
          canWrite={editable}
          refs={refs}
          onRowsChange={onValuesChange}
          showPending={dirty || !record}
          formulaCell={(index, lineId, fieldKey) => {
            const saved = lineId ? savedLineById.get(lineId) : undefined;
            return formulaCell(preview, table.entity.key, index, fieldKey, {
              data: saved?.data,
              errors: saved?.formula_errors,
            });
          }}
        />
      ) : null;
    }
    switch (code) {
      case FORM_BLOCKS.recordNo:
        return showsRecordNo(entity, record) ? (
          <Form.Item label="單號">
            <Input
              aria-label="單號"
              readOnly
              disabled
              style={{ maxWidth: 320 }}
              value={record?.record_no ?? ''}
              placeholder={RECORD_NO_PENDING}
            />
          </Form.Item>
        ) : null;
      case FORM_BLOCKS.allowanceAvailable:
        // 折讓的帶入工具：選擇發票（憑證）後帶入，並顯示可折讓金額
        if (!editable) {
          return null;
        }
        if (entityKey === SALES_ALLOWANCE) {
          return (
            <SalesAllowanceAutofill details={details} isNew={!record} onChange={onValuesChange} />
          );
        }
        return entityKey === PURCHASE_ALLOWANCE ? (
          <PurchaseAllowanceAutofill isNew={!record} onChange={onValuesChange} />
        ) : null;
      case FORM_BLOCKS.shipmentPicker:
        return editable && entityKey === SALES_INVOICE ? (
          <ShipmentPicker details={details} onChange={onValuesChange} />
        ) : null;
      case FORM_BLOCKS.receiptPicker:
        return editable && entityKey === PURCHASE_INVOICE ? (
          <ReceiptPicker details={details} onChange={onValuesChange} />
        ) : null;
      case FORM_BLOCKS.usedLots:
        return (isShipment || entityKey === SALES_RETURN) &&
          record &&
          (status === 'posted' || status === 'void') ? (
          <UsedLotsTable record={record} docType={isShipment ? SHIPMENT : SALES_RETURN} />
        ) : null;
      default:
        return null;
    }
  };

  const title = record ? entity.label : `新增${entity.label}`;

  const Frame = panel ? PanelFrame : Page;

  return (
    <Frame
      icon={entityIcon(entity.icon)}
      title={panel && record ? `編輯：${record.title}` : title}
      tags={
        (status || dirty) && (
          <>
            {status && <DocumentStatusTag status={status} entityKey={entityKey} />}
            {dirty && <StatusTag tone="warning">未存檔</StatusTag>}
          </>
        )
      }
      actions={
        <Space>
          {actions.map((action) => (
            <Tooltip key={action} title={dirty ? SAVE_BEFORE_ACTION : undefined}>
              <Button
                type={action === 'post' || action === 'confirm' ? 'primary' : 'default'}
                danger={action === 'void'}
                icon={ACTION_ICONS[action]}
                disabled={dirty}
                loading={mutations[action].isPending}
                onClick={() => !actionPending && runAction(action)}
              >
                {actionLabel(action)}
              </Button>
            </Tooltip>
          ))}
          {allowanceEntity && record && status === 'posted' && canWrite && (
            <Button
              icon={<AuditOutlined />}
              onClick={() => navigate(`${newRecordPath(allowanceEntity)}?invoice=${record.id}`)}
            >
              開立折讓
            </Button>
          )}
          {panel ? (
            <Button icon={<CloseOutlined />} onClick={panel.onCancel}>
              取消
            </Button>
          ) : (
            <Button
              icon={<ArrowLeftOutlined />}
              onClick={() => navigate(recordListPath(entityKey))}
            >
              返回列表
            </Button>
          )}
        </Space>
      }
    >
      {actionError && (
        <Alert
          showIcon
          closable
          type="error"
          style={{ marginBottom: 16 }}
          message={actionError.title}
          description={
            actionError.items.length > 0 ? (
              <ul style={{ margin: 0, paddingInlineStart: 20 }}>
                {actionError.items.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            ) : undefined
          }
          action={
            actionError.reload ? (
              <Button icon={<ReloadOutlined />} onClick={onReload}>
                重新載入
              </Button>
            ) : undefined
          }
          onClose={() => setActionError(undefined)}
        />
      )}
      {isDocument && record && status && status !== 'draft' && (
        <Typography.Paragraph type="secondary">
          {invoicingStatusNote(
            entityKey,
            documentStatusNote(entityKey, status, record.document, formatTaipei),
          )}
        </Typography.Paragraph>
      )}
      {/* 出貨單的發票號碼（§8.11「查詢 API」）：作廢前要先作廢發票 */}
      {isShipment && record && (status === 'posted' || status === 'void') && (
        <InvoiceLinksNote entityKey={SHIPMENT} record={record} />
      )}
      {entityKey === GOODS_RECEIPT && record && (status === 'posted' || status === 'void') && (
        <InvoiceLinksNote entityKey={GOODS_RECEIPT} record={record} label="進項憑證" />
      )}
      {conflict && (
        <Alert
          showIcon
          type="warning"
          style={{ marginBottom: 16 }}
          message={RECORD_CONFLICT_MESSAGE}
          description="重新載入會取得最新的資料，並放棄目前未存檔的修改。"
          action={
            <Button icon={<ReloadOutlined />} onClick={onReload}>
              重新載入
            </Button>
          }
        />
      )}
      {displayed.length === 0 && details.length === 0 ? (
        <EmptyState title="此資料類別沒有可以輸入的欄位" description="請先在「資料類別」新增欄位" />
      ) : (
        <Form<FormValues>
          form={form}
          layout="vertical"
          initialValues={initialValues}
          disabled={!editable}
          onValuesChange={onValuesChange}
          onFinish={onFinish}
          onFinishFailed={({ errorFields }) =>
            setRevealFields(errorFields.map((e) => String(e.name[0])))
          }
        >
          {/* 帶入工具（沒有畫面，只監聽欄位變動），不受版面影響（§7.7「系統區塊」） */}
          {editable && entityKey === PURCHASE_RETURN && !record && (
            <PurchaseReturnAutofill details={details} onChange={onValuesChange} />
          )}
          {editable && entityKey === SALES_RETURN && !record && (
            <SalesReturnAutofill details={details} onChange={onValuesChange} />
          )}
          {editable && entityKey === SALES_INVOICE && (
            <SalesInvoiceAutofill onChange={onValuesChange} />
          )}
          {editable && entityKey === PURCHASE_INVOICE && (
            <PurchaseInvoiceAutofill onChange={onValuesChange} />
          )}
          {editable && isSales && (
            <SalesAutofill entityKey={entityKey} details={details} onChange={onValuesChange} />
          )}
          {editable && priced && (
            <PurchasingAutofill
              entityKey={entityKey}
              isNew={!record}
              fields={fields}
              details={details}
              onChange={onValuesChange}
            />
          )}
          <ShipmentAllocationContext.Provider value={allocation}>
            <FormLayoutRenderer
              config={formLayout}
              reveal={revealFields}
              renderField={(fieldKey) => {
                const field = displayedByKey.get(fieldKey);
                if (!field) {
                  return null;
                }
                const item = renderFieldItem(field);
                // 期別說明跟著期別欄位（§8.11）
                return invoicing && fieldKey === INVOICING_FIELDS.taxPeriod ? (
                  <>
                    {item}
                    <TaxPeriodNote />
                  </>
                ) : (
                  item
                );
              }}
              renderBlock={renderBlock}
            />
          </ShipmentAllocationContext.Provider>
          {editable && (
            <Form.Item>
              <Button
                type="primary"
                htmlType="submit"
                icon={<SaveOutlined />}
                loading={saving}
                disabled={record !== undefined && !dirty}
              >
                儲存
              </Button>
            </Form.Item>
          )}
        </Form>
      )}
      {invoicing && record && (
        <VoidInvoiceModal
          open={voidOpen}
          title={`作廢這張${entity.label}`}
          period={record.data[INVOICING_FIELDS.taxPeriod]}
          pending={voidInvoice.isPending}
          onOk={runVoidInvoice}
          onCancel={() => setVoidOpen(false)}
        />
      )}
    </Frame>
  );
}

/** 分割檢視右側的表單：存檔成功、取消時由外部處理。 */
export interface RecordFormPanelActions {
  /** 存檔成功（新增或修改）後，`saved` 為伺服器回傳的資料。 */
  onSaved: (saved: DataRecord) => void;
  onCancel: () => void;
}

/** 分割檢視右側的表單外框：與 `Page` 相同的參數，標題列較小、不另外包卡片。 */
function PanelFrame({
  title,
  tags,
  actions,
  children,
}: PageHeaderProps & { children?: ReactNode }) {
  return (
    <div data-testid="record-form-panel">
      <div className="preview-header">
        <Space size={8} wrap>
          <Typography.Title level={5} style={{ margin: 0 }}>
            {title}
          </Typography.Title>
          {tags}
        </Space>
        {actions}
      </div>
      {children}
    </div>
  );
}

/**
 * 載入欄位定義、明細子表、表單版面與資料後顯示 {@link RecordForm}。`id` 為 `undefined` 時是新增。
 */
function RecordFormLoader({
  entityKey,
  id,
  panel,
  onDirtyChange,
}: {
  entityKey: string;
  id: string | undefined;
  panel?: RecordFormPanelActions;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const key = entityKey;
  const Frame = panel ? PanelFrame : Page;
  const entity = useEntity(key);
  const entities = useEntities();
  const record = useRecord(key, id);
  const detailEntities = useMemo(
    () =>
      entity.data && entities.data
        ? detailEntitiesOf(entities.data, entity.data.entity)
        : undefined,
    [entity.data, entities.data],
  );
  const detailDefs = useEntityDetails(detailEntities);
  // 表單版面（§7.7）：讀取失敗時依欄位定義產生，不影響輸入
  const formLayout = useFormLayout(key);
  // 新增時的預設值（盤點調整單的調整日期預設今天）
  const fields = useMemo(
    () => (entity.data ? withFormDefaults(key, entity.data.fields, taipeiToday()) : undefined),
    [key, entity.data],
  );
  const details = useMemo<DetailTable[] | undefined>(
    () =>
      detailDefs.data?.map((d) => ({
        entity: d.entity,
        fields: lineFormFields(d.entity.key, d.fields),
        formulas: formulaFields(d.fields),
      })),
    [detailDefs.data],
  );

  const error = entity.error ?? entities.error ?? detailDefs.error ?? record.error;
  if (error) {
    return (
      <Frame title="資料">
        <Alert showIcon type="error" message={errorMessage(error)} />
      </Frame>
    );
  }
  if (
    !entity.data ||
    !details ||
    !detailDefs.data ||
    formLayout.isPending ||
    (id !== undefined && !record.data)
  ) {
    return panel ? (
      <div style={{ padding: 24, textAlign: 'center' }}>
        <Spin />
      </div>
    ) : (
      <Page title="資料" loading />
    );
  }
  const detail = entity.data;
  if (!detail.entity.is_active) {
    return (
      <Frame title={detail.entity.label}>
        <EmptyState kind="disabled" title="此資料類別已停用" description="請洽管理者重新啟用" />
      </Frame>
    );
  }
  return (
    <RecordForm
      // 資料重新載入或存檔後，以新的資料重設表單
      key={`${key}:${id ?? 'new'}:${record.dataUpdatedAt}`}
      entity={detail.entity}
      fields={fields ?? detail.fields}
      details={details}
      detailDefs={detailDefs.data}
      layout={formLayout.data?.config}
      record={record.data}
      onReload={() => void record.refetch()}
      panel={panel}
      onDirtyChange={onDirtyChange}
    />
  );
}

/**
 * 分割檢視（§8.7）右側的新增、編輯表單。未存檔的提醒由分割檢視的 {@link useUnsavedGuard} 處理。
 */
export function RecordFormPanel({
  entityKey,
  id,
  onSaved,
  onCancel,
  onDirtyChange,
}: RecordFormPanelActions & {
  entityKey: string;
  id: string | undefined;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const panel = useMemo(() => ({ onSaved, onCancel }), [onSaved, onCancel]);
  return (
    <RecordFormLoader entityKey={entityKey} id={id} panel={panel} onDirtyChange={onDirtyChange} />
  );
}

/** 全頁表單：有未存檔的修改時，換頁、關閉視窗前提醒。 */
function FullPageRecordForm({ entityKey, id }: { entityKey: string; id: string | undefined }) {
  const guard = useUnsavedGuard();
  return (
    <>
      <RecordFormLoader entityKey={entityKey} id={id} onDirtyChange={guard.setDirty} />
      {guard.blockerModal}
    </>
  );
}

/**
 * 新增資料（`/entities/:key/new`，需要 `record.write`）與檢視、編輯資料
 * （`/entities/:key/:id`，需要 `record.read`；沒有 `record.write` 時只能檢視）。
 *
 * 表單依欄位定義產生：有效且設定為顯示的欄位，依表單版面（§7.7）排列；沒有設定版面時依欄位排序。
 * 驗證規則與伺服器相同。
 *
 * 使用分割檢視的資料類別（客戶、廠商，§8.7）導向分割畫面：`/:id` 選取那筆，`/new` 開啟新增。
 */
export function RecordFormPage() {
  const { key = '', id } = useParams();
  if (usesSplitView(key)) {
    return (
      <Navigate
        replace
        to={splitViewPath(key, id === undefined ? { mode: 'new' } : { selected: id })}
      />
    );
  }
  return <FullPageRecordForm key={`${key}:${id ?? 'new'}`} entityKey={key} id={id} />;
}
