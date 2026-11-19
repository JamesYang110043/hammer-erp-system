import { FileAddOutlined, LinkOutlined } from '@ant-design/icons';
import { useQueryClient } from '@tanstack/react-query';
import {
  Alert,
  App,
  Button,
  DatePicker,
  Form,
  Input,
  Modal,
  Space,
  Table,
  Tag,
  Typography,
} from 'antd';
import dayjs from 'dayjs';
import type { Dayjs } from 'dayjs';
import { useEffect, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';

import { useTaxRates } from '../../api/currencies';
import {
  recordQuery,
  useInvoiceLinks,
  usePostedAllowances,
  useRecordById,
  useUninvoicedReceipts,
  useUninvoicedShipments,
} from '../../api/invoicing';
import type {
  Record as DataRecord,
  UninvoicedReceipt_Serialize,
  UninvoicedShipment_Serialize,
} from '../../bindings';
import { recordPath } from '../../modules';
import { DOCUMENT_STATUS_LABELS, documentStatus } from '../../records/documents';
import { initialFormValues } from '../../records/form';
import {
  CUSTOMS,
  DUPLICATE,
  INVOICING_FIELDS as F,
  PURCHASE_ALLOWANCE,
  PURCHASE_INVOICE,
  PURCHASE_INVOICE_RECEIPT,
  SALES_ALLOWANCE,
  SALES_ALLOWANCE_LINE,
  SALES_INVOICE,
  SALES_INVOICE_LINE,
  SALES_INVOICE_SHIPMENT,
  availableAllowance,
  crossPeriodWarning,
  dateText,
  linesFromSource,
  moneyText,
  suggestedInvoiceAmounts,
  postedLabel,
  sumAmounts,
  taxPeriodLabel,
} from '../../records/invoicing';
import { LINES_FORM_KEY } from '../../records/lines';
import type { DetailTable, LineFormValues } from '../../records/lines';
import { taipeiToday } from '../../utils/time';

/**
 * 發票模組的畫面輔助（docs/architecture.md §8.11）：銷項發票的買受人帶入與選擇出貨單、銷貨折讓的
 * 可折讓金額與銷貨退回帶入、出貨單上的發票號碼、作廢對話框。
 */

function textOf(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

/** 台幣金額的顯示：整數、千分位（`1070.0000` → `1,070`）。 */
const money = (raw: unknown) => moneyText(raw);

/**
 * 銷項發票：選擇客戶（新增或更換）時帶入買受人統一編號、抬頭（沒有發票抬頭時用公司名稱）與地址
 * （沒有發票地址時用聯絡地址），之後可以修改（快照，§8.11「買受人資訊」）。
 */
export function SalesInvoiceAutofill({ onChange }: { onChange: () => void }) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const customerId = textOf(Form.useWatch(F.customer, form));
  const prev = useRef(textOf(form.getFieldValue(F.customer)));

  useEffect(() => {
    if (customerId !== textOf(form.getFieldValue(F.customer))) {
      return;
    }
    const before = prev.current;
    prev.current = customerId;
    if (!customerId || customerId === before) {
      return;
    }
    void queryClient
      .fetchQuery(recordQuery('customer', customerId))
      .then((customer) => {
        const d = customer.data;
        form.setFieldsValue({
          [F.buyerTaxId]: textOf(d.tax_id),
          [F.buyerName]: textOf(d.invoice_title) ?? textOf(d.name),
          [F.buyerAddress]: textOf(d.invoice_address) ?? textOf(d.address),
        });
        onChange();
      })
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [customerId]);

  return null;
}

/**
 * 銷項發票的「選擇出貨單」：列出客戶尚未開立發票的已過帳出貨單，勾選後加入關聯並帶入明細（單價扣掉
 * 折扣、依三聯式或二聯式換算）。取消勾選時詢問是否以勾選的出貨單重新帶入明細。
 */
export function ShipmentPicker({
  details,
  onChange,
}: {
  details: readonly DetailTable[];
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const { message, modal } = App.useApp();
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const customerId = textOf(Form.useWatch(F.customer, form));
  const shipments = useUninvoicedShipments(customerId, open);
  const taxRates = useTaxRates();
  const linkTable = details.find((d) => d.entity.key === SALES_INVOICE_SHIPMENT);
  const lineTable = details.find((d) => d.entity.key === SALES_INVOICE_LINE);
  const linkPath = [LINES_FORM_KEY, SALES_INVOICE_SHIPMENT];
  const linePath = [LINES_FORM_KEY, SALES_INVOICE_LINE];

  const linked = (): string[] =>
    ((form.getFieldValue(linkPath) ?? []) as LineFormValues[]).flatMap((r) => {
      const id = textOf(r[F.shipment]);
      return id ? [id] : [];
    });

  const openPicker = () => {
    if (!customerId) {
      message.warning('請先選擇客戶');
      return;
    }
    setSelected(linked());
    setOpen(true);
  };

  const apply = async () => {
    if (!linkTable || !lineTable) {
      return;
    }
    const current = linked();
    const added = selected.filter((id) => !current.includes(id));
    const removed = current.filter((id) => !selected.includes(id));
    const duplicate = form.getFieldValue(F.invoiceType) === DUPLICATE;
    const rate = (code: unknown) => taxRates.data?.find((t) => t.code === code)?.rate ?? '0';
    const fetchLines = async (ids: string[]) => {
      const records = await Promise.all(
        ids.map((id) => queryClient.fetchQuery(recordQuery('shipment', id))),
      );
      return records;
    };
    let shipmentsRead: DataRecord[];
    try {
      shipmentsRead = await fetchLines(removed.length > 0 ? selected : added);
    } catch {
      message.error('無法讀取出貨單');
      return;
    }
    const toRows = (records: DataRecord[]) =>
      records.flatMap((s) =>
        linesFromSource(
          s,
          'shipment_line',
          duplicate,
          rate(form.getFieldValue(F.taxType) ?? s.data.tax_type),
        ).map((data) => initialFormValues(lineTable.fields, data)),
      );

    // 關聯：保留仍勾選的列（有 id 的不會被刪除後重建），加上新勾選的
    const keptLinks = ((form.getFieldValue(linkPath) ?? []) as LineFormValues[]).filter((r) =>
      selected.includes(textOf(r[F.shipment]) ?? ''),
    );
    form.setFieldValue(linkPath, [
      ...keptLinks,
      ...added.map((id) => initialFormValues(linkTable.fields, { [F.shipment]: id })),
    ]);
    if (!textOf(form.getFieldValue(F.taxType)) && shipmentsRead[0]) {
      form.setFieldValue(F.taxType, shipmentsRead[0].data.tax_type);
    }
    setOpen(false);

    const currentLines = (form.getFieldValue(linePath) ?? []) as LineFormValues[];
    const append = () => {
      const newOnes = shipmentsRead.filter((s) => added.includes(s.id));
      form.setFieldValue(linePath, [...currentLines, ...toRows(newOnes)]);
      onChange();
    };
    if (removed.length > 0 && currentLines.length > 0) {
      modal.confirm({
        title: '要依勾選的出貨單重新帶入明細嗎？',
        content: `已取消勾選 ${removed.length} 張出貨單。重新帶入會以勾選的出貨單明細取代目前的 ${currentLines.length} 筆明細。`,
        okText: '重新帶入',
        cancelText: '保留目前的明細',
        onOk: () => {
          form.setFieldValue(linePath, toRows(shipmentsRead));
          onChange();
        },
        onCancel: append,
      });
      return;
    }
    append();
    if (added.length > 0) {
      message.success(`已加入 ${added.length} 張出貨單的明細，可以修改、合併或新增列`);
    }
  };

  return (
    <>
      <Space style={{ marginBottom: 16 }}>
        <Button icon={<FileAddOutlined />} onClick={openPicker}>
          選擇出貨單
        </Button>
        <Typography.Text type="secondary">
          勾選尚未開立發票的出貨單，帶入明細後可以自由修改
        </Typography.Text>
      </Space>
      <Modal
        open={open}
        title="選擇出貨單"
        okText="帶入"
        cancelText="取消"
        width={760}
        onOk={() => void apply()}
        onCancel={() => setOpen(false)}
      >
        <Table<UninvoicedShipment_Serialize>
          aria-label="尚未開立發票的出貨單"
          size="small"
          rowKey="id"
          loading={shipments.isLoading}
          pagination={false}
          dataSource={shipments.data?.items ?? []}
          locale={{ emptyText: '這個客戶沒有尚未開立發票的出貨單' }}
          rowSelection={{
            selectedRowKeys: selected,
            onChange: (keys) => setSelected(keys as string[]),
          }}
          columns={[
            { title: '單號', dataIndex: 'record_no' },
            { title: '出貨日期', dataIndex: 'ship_date' },
            {
              title: '總計',
              dataIndex: 'total',
              align: 'right',
              render: (v: string) => money(v),
            },
            {
              title: '退回後淨額',
              dataIndex: 'net_total',
              align: 'right',
              render: (v: string) => money(v),
            },
          ]}
        />
      </Modal>
    </>
  );
}

/** 進項憑證：選擇廠商（新增或更換）時帶入賣方統一編號（快照，§8.11「買受人資訊」）。 */
export function PurchaseInvoiceAutofill({ onChange }: { onChange: () => void }) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const supplierId = textOf(Form.useWatch(F.supplier, form));
  const prev = useRef(textOf(form.getFieldValue(F.supplier)));

  useEffect(() => {
    if (supplierId !== textOf(form.getFieldValue(F.supplier))) {
      return;
    }
    const before = prev.current;
    prev.current = supplierId;
    if (!supplierId || supplierId === before) {
      return;
    }
    void queryClient
      .fetchQuery(recordQuery('supplier', supplierId))
      .then((supplier) => {
        form.setFieldValue(F.sellerTaxId, textOf(supplier.data.tax_id));
        onChange();
      })
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [supplierId]);

  return null;
}

/**
 * 進項憑證的「選擇進貨單」：列出廠商尚未取得該類憑證（發票類或海關繳納證）的已過帳進貨單，勾選後加入
 * 關聯。還沒有輸入金額、且勾選的進貨單都是台幣時，依進貨單的台幣總計帶入未稅金額與稅額（請依憑證修改）。
 */
export function ReceiptPicker({
  details,
  onChange,
}: {
  details: readonly DetailTable[];
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const { message } = App.useApp();
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const supplierId = textOf(Form.useWatch(F.supplier, form));
  const customs = Form.useWatch(F.docType, form) === CUSTOMS;
  const receipts = useUninvoicedReceipts(supplierId, customs, open);
  const taxRates = useTaxRates();
  const linkTable = details.find((d) => d.entity.key === PURCHASE_INVOICE_RECEIPT);
  const linkPath = [LINES_FORM_KEY, PURCHASE_INVOICE_RECEIPT];

  const linked = (): string[] =>
    ((form.getFieldValue(linkPath) ?? []) as LineFormValues[]).flatMap((r) => {
      const id = textOf(r[F.goodsReceipt]);
      return id ? [id] : [];
    });

  const openPicker = () => {
    if (!supplierId) {
      message.warning('請先選擇廠商');
      return;
    }
    setSelected(linked());
    setOpen(true);
  };

  const apply = () => {
    if (!linkTable) {
      return;
    }
    const current = linked();
    const added = selected.filter((id) => !current.includes(id));
    const keptLinks = ((form.getFieldValue(linkPath) ?? []) as LineFormValues[]).filter((r) =>
      selected.includes(textOf(r[F.goodsReceipt]) ?? ''),
    );
    form.setFieldValue(linkPath, [
      ...keptLinks,
      ...added.map((id) => initialFormValues(linkTable.fields, { [F.goodsReceipt]: id })),
    ]);
    setOpen(false);

    const rows = (receipts.data?.items ?? []).filter((r) => selected.includes(r.id));
    const hasAmounts =
      textOf(form.getFieldValue(F.subtotal)) !== undefined ||
      textOf(form.getFieldValue(F.taxAmount)) !== undefined;
    const rate = taxRates.data?.find((t) => t.code === form.getFieldValue(F.taxType))?.rate;
    const suggested =
      !customs && !hasAmounts && rate !== undefined
        ? suggestedInvoiceAmounts(rows, rate)
        : undefined;
    if (suggested) {
      form.setFieldsValue({
        [F.subtotal]: suggested.subtotal,
        [F.taxAmount]: suggested.tax_amount,
      });
      message.success('已依進貨單的台幣總計帶入金額，請依憑證修改');
    } else if (added.length > 0) {
      message.success(`已加入 ${added.length} 張進貨單，請依憑證輸入金額`);
    }
    onChange();
  };

  return (
    <>
      <Space style={{ marginBottom: 16 }}>
        <Button icon={<FileAddOutlined />} onClick={openPicker}>
          選擇進貨單
        </Button>
        <Typography.Text type="secondary">
          {customs ? '勾選尚未取得海關繳納證的進貨單' : '勾選尚未取得發票的進貨單'}
        </Typography.Text>
      </Space>
      <Modal
        open={open}
        title="選擇進貨單"
        okText="帶入"
        cancelText="取消"
        width={820}
        onOk={apply}
        onCancel={() => setOpen(false)}
      >
        <Table<UninvoicedReceipt_Serialize>
          aria-label="尚未取得憑證的進貨單"
          size="small"
          rowKey="id"
          loading={receipts.isLoading}
          pagination={false}
          dataSource={receipts.data?.items ?? []}
          locale={{ emptyText: '這個廠商沒有尚未取得憑證的進貨單' }}
          rowSelection={{
            selectedRowKeys: selected,
            onChange: (keys) => setSelected(keys as string[]),
          }}
          columns={[
            { title: '單號', dataIndex: 'record_no' },
            { title: '進貨日期', dataIndex: 'receipt_date' },
            { title: '幣別', dataIndex: 'currency_code' },
            {
              title: '總計',
              dataIndex: 'total',
              align: 'right',
              render: (v: string, r) => (r.currency_code === 'TWD' ? money(v) : v),
            },
            {
              title: '台幣總計',
              dataIndex: 'total_twd',
              align: 'right',
              render: (v: string | null) => (v ? money(v) : '—'),
            },
          ]}
        />
      </Modal>
    </>
  );
}

/** 期別的說明（發票、折讓的日期下方）。 */
export function TaxPeriodNote() {
  const form = Form.useFormInstance();
  const period = Form.useWatch(F.taxPeriod, form);
  const label = taxPeriodLabel(period);
  return label ? (
    <Typography.Paragraph type="secondary" style={{ marginTop: -8 }}>
      期別：{label}
    </Typography.Paragraph>
  ) : null;
}

/**
 * 銷貨折讓：從發票開立時以網址的 `invoice` 帶入原發票；顯示可折讓金額；選擇銷貨退回且還沒有明細時
 * 帶入退回的明細（單價換算為未稅）。
 */
export function SalesAllowanceAutofill({
  details,
  isNew,
  onChange,
}: {
  details: readonly DetailTable[];
  isNew: boolean;
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const { message } = App.useApp();
  const taxRates = useTaxRates();
  const invoiceId = useAllowanceHeader(SALES_ALLOWANCE_KIND, F.customer, isNew, onChange);
  const returnId = textOf(Form.useWatch(F.salesReturn, form));
  const prevReturn = useRef(textOf(form.getFieldValue(F.salesReturn)));

  useEffect(() => {
    if (returnId !== textOf(form.getFieldValue(F.salesReturn))) {
      return;
    }
    const before = prevReturn.current;
    prevReturn.current = returnId;
    const table = details.find((d) => d.entity.key === SALES_ALLOWANCE_LINE);
    const path = [LINES_FORM_KEY, SALES_ALLOWANCE_LINE];
    if (!returnId || returnId === before || !table) {
      return;
    }
    if (((form.getFieldValue(path) ?? []) as unknown[]).length > 0) {
      return;
    }
    void queryClient
      .fetchQuery(recordQuery('sales_return', returnId))
      .then((ret) => {
        const rate = taxRates.data?.find((t) => t.code === ret.data.tax_type)?.rate ?? '0';
        const lines = linesFromSource(ret, 'sales_return_line', false, rate);
        form.setFieldValue(
          path,
          lines.map((data) => initialFormValues(table.fields, data)),
        );
        onChange();
        message.success(
          `已帶入 ${ret.record_no ?? '銷貨退回'} 的 ${lines.length} 筆明細（未稅單價）`,
        );
      })
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [returnId]);

  return <AllowanceAvailable kind={SALES_ALLOWANCE_KIND} invoiceId={invoiceId} />;
}

/**
 * 進貨折讓：與銷貨折讓相同帶入原憑證、廠商、稅別並顯示可折讓金額；選擇進貨退回且還沒有輸入金額時，
 * 台幣的退回單帶入未稅金額與稅額（外幣不帶入，依折讓證明單輸入）。
 */
export function PurchaseAllowanceAutofill({
  isNew,
  onChange,
}: {
  isNew: boolean;
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const { message } = App.useApp();
  const invoiceId = useAllowanceHeader(PURCHASE_ALLOWANCE_KIND, F.supplier, isNew, onChange);
  const returnId = textOf(Form.useWatch(F.purchaseReturn, form));
  const prevReturn = useRef(textOf(form.getFieldValue(F.purchaseReturn)));

  useEffect(() => {
    if (returnId !== textOf(form.getFieldValue(F.purchaseReturn))) {
      return;
    }
    const before = prevReturn.current;
    prevReturn.current = returnId;
    if (!returnId || returnId === before) {
      return;
    }
    if (textOf(form.getFieldValue(F.subtotal)) || textOf(form.getFieldValue(F.taxAmount))) {
      return;
    }
    void queryClient
      .fetchQuery(recordQuery('purchase_return', returnId))
      .then((ret) => {
        if (ret.data.currency_code !== 'TWD') {
          message.info('外幣的進貨退回不帶入金額，請依折讓證明單輸入台幣金額');
          return;
        }
        form.setFieldsValue({
          [F.subtotal]: ret.data.subtotal,
          [F.taxAmount]: ret.data.tax_amount,
        });
        onChange();
        message.success(`已帶入 ${ret.record_no ?? '進貨退回'} 的金額，請依折讓證明單確認`);
      })
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [returnId]);

  return <AllowanceAvailable kind={PURCHASE_ALLOWANCE_KIND} invoiceId={invoiceId} />;
}

/** 折讓與原發票（憑證）的對應。 */
interface AllowanceKind {
  invoiceEntity: string;
  allowanceEntity: string;
  invoiceField: string;
  /** 訊息中的「發票」或「憑證」。 */
  word: string;
}

const SALES_ALLOWANCE_KIND: AllowanceKind = {
  invoiceEntity: SALES_INVOICE,
  allowanceEntity: SALES_ALLOWANCE,
  invoiceField: F.salesInvoice,
  word: '發票',
};

const PURCHASE_ALLOWANCE_KIND: AllowanceKind = {
  invoiceEntity: PURCHASE_INVOICE,
  allowanceEntity: PURCHASE_ALLOWANCE,
  invoiceField: F.purchaseInvoice,
  word: '憑證',
};

/** 原發票（憑證）的可折讓金額（總計 − 已開立折讓的合計）。 */
function AllowanceAvailable({
  kind,
  invoiceId,
}: {
  kind: AllowanceKind;
  invoiceId: string | undefined;
}) {
  const invoice = useRecordById(kind.invoiceEntity, invoiceId);
  const posted = usePostedAllowances(kind.allowanceEntity, kind.invoiceField, invoiceId);
  if (!invoice.data || !posted.data) {
    return null;
  }
  const status = documentStatus(invoice.data);
  const warning =
    status === 'void'
      ? `原${kind.word}已作廢，不能開立折讓`
      : status !== 'posted'
        ? `原${kind.word}${kind.word === '憑證' ? '尚未登錄' : '尚未開立'}，不能開立折讓`
        : invoice.data.data[F.docType] === CUSTOMS
          ? '海關繳納證不可折讓'
          : undefined;
  if (warning) {
    return <Alert showIcon type="warning" style={{ marginBottom: 16 }} message={warning} />;
  }
  const totals = posted.data.items.map((a) => a.data.total);
  const available = availableAllowance(invoice.data.data.total, totals) ?? '0';
  const used = sumAmounts(totals);
  return (
    <Alert
      showIcon
      type="info"
      style={{ marginBottom: 16 }}
      message={`可折讓金額 ${money(available)}`}
      description={`${kind.word} ${String(invoice.data.data.invoice_no ?? '')} 總計 ${money(
        String(invoice.data.data.total ?? '0'),
      )}，已折讓 ${money(used)}`}
    />
  );
}

/**
 * 折讓共通：從原發票開立時以網址的 `invoice` 帶入原發票；原發票的客戶（廠商）與稅別先帶入（伺服器存檔時
 * 也會帶入），讓金額預覽可以計算稅額。回傳目前的原發票 id。
 */
function useAllowanceHeader(
  kind: AllowanceKind,
  partyField: string,
  isNew: boolean,
  onChange: () => void,
): string | undefined {
  const form = Form.useFormInstance();
  const [params] = useSearchParams();
  const invoiceId = textOf(Form.useWatch(kind.invoiceField, form));

  useEffect(() => {
    const fromUrl = params.get('invoice');
    if (isNew && fromUrl && !textOf(form.getFieldValue(kind.invoiceField))) {
      form.setFieldValue(kind.invoiceField, fromUrl);
      onChange();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const invoice = useRecordById(kind.invoiceEntity, invoiceId);
  useEffect(() => {
    const d = invoice.data?.data;
    if (!d) {
      return;
    }
    if (
      form.getFieldValue(partyField) !== d[partyField] ||
      form.getFieldValue(F.taxType) !== d.tax_type
    ) {
      form.setFieldsValue({ [partyField]: d[partyField], [F.taxType]: d.tax_type });
      onChange();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [invoice.data]);

  return invoiceId;
}

/** 出貨單（進貨單）上的發票（進項憑證）號碼：關聯的發票、憑證，含已作廢。 */
export function InvoiceLinksNote({
  entityKey,
  record,
  label = '發票',
}: {
  entityKey: string;
  record: DataRecord;
  /** 「發票」或「進項憑證」。 */
  label?: string;
}) {
  const links = useInvoiceLinks(entityKey, record.id);
  const items = links.data ?? [];
  return (
    <Typography.Paragraph>
      <Space wrap>
        <span>
          <LinkOutlined /> {label}：
        </span>
        {links.isLoading ? (
          <Typography.Text type="secondary">讀取中</Typography.Text>
        ) : items.length === 0 ? (
          <Typography.Text type="secondary">
            {label === '發票' ? '尚未開立發票' : `尚未取得${label}`}
          </Typography.Text>
        ) : (
          items.map((link) => (
            <Link key={link.record_id} to={recordPath(link.entity_key, link.record_id)}>
              <Tag
                color={
                  link.status === 'posted' ? 'green' : link.status === 'void' ? 'red' : 'default'
                }
              >
                {link.invoice_no ?? link.record_no}（
                {link.status === 'posted'
                  ? (postedLabel(link.entity_key) ?? DOCUMENT_STATUS_LABELS.posted)
                  : DOCUMENT_STATUS_LABELS[link.status]}
                ）
              </Tag>
            </Link>
          ))
        )}
      </Space>
    </Typography.Paragraph>
  );
}

/** 作廢對話框的輸入。 */
export interface VoidDetails {
  voidDate: string;
  voidReason: string;
}

/**
 * 發票模組的作廢對話框：作廢日期（預設今天）、作廢原因（必填）；作廢日期與單據不同期時顯示警告，
 * 仍可作廢（§8.11「開立與作廢」）。
 */
export function VoidInvoiceModal({
  open,
  title,
  period,
  pending,
  onOk,
  onCancel,
}: {
  open: boolean;
  title: string;
  /** 單據的期別（`tax_period`）。 */
  period: unknown;
  pending: boolean;
  onOk: (details: VoidDetails) => void;
  onCancel: () => void;
}) {
  const [form] = Form.useForm<{ void_date?: Dayjs; void_reason?: string }>();
  const date = Form.useWatch('void_date', form);
  const warning = crossPeriodWarning(dateText(date), period);

  return (
    <Modal
      open={open}
      title={title}
      okText="作廢"
      okButtonProps={{ danger: true, loading: pending }}
      cancelText="取消"
      destroyOnHidden
      onCancel={onCancel}
      onOk={() =>
        void form.validateFields().then((v) =>
          onOk({
            voidDate: dateText(v.void_date) ?? taipeiToday(),
            voidReason: (v.void_reason ?? '').trim(),
          }),
        )
      }
    >
      <Typography.Paragraph type="secondary">作廢後單據不能再恢復。</Typography.Paragraph>
      <Form
        form={form}
        layout="vertical"
        preserve={false}
        initialValues={{ void_date: dayjs(taipeiToday()), void_reason: '' }}
      >
        <Form.Item
          name="void_date"
          label="作廢日期"
          rules={[{ required: true, message: '請選擇作廢日期' }]}
        >
          <DatePicker aria-label="作廢日期" allowClear={false} />
        </Form.Item>
        <Form.Item
          name="void_reason"
          label="作廢原因"
          required
          rules={[
            {
              validator: (_, v: unknown) =>
                typeof v === 'string' && v.trim() !== ''
                  ? Promise.resolve()
                  : Promise.reject(new Error('請輸入作廢原因')),
            },
          ]}
        >
          <Input.TextArea aria-label="作廢原因" rows={2} maxLength={200} />
        </Form.Item>
      </Form>
      {warning && <Alert showIcon type="warning" message={warning} />}
    </Modal>
  );
}
