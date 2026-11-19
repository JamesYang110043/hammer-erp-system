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
  useUninvoicedShipments,
} from '../../api/invoicing';
import type { Record as DataRecord, UninvoicedShipment_Serialize } from '../../bindings';
import { recordPath } from '../../modules';
import { DOCUMENT_STATUS_LABELS, documentStatus } from '../../records/documents';
import { initialFormValues } from '../../records/form';
import {
  DUPLICATE,
  INVOICING_FIELDS as F,
  SALES_ALLOWANCE_LINE,
  SALES_INVOICE,
  SALES_INVOICE_LINE,
  SALES_INVOICE_SHIPMENT,
  availableAllowance,
  crossPeriodWarning,
  dateText,
  linesFromSource,
  moneyText,
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
  const [params] = useSearchParams();
  const taxRates = useTaxRates();
  const invoiceId = textOf(Form.useWatch(F.salesInvoice, form));
  const returnId = textOf(Form.useWatch(F.salesReturn, form));
  const prevReturn = useRef(textOf(form.getFieldValue(F.salesReturn)));

  useEffect(() => {
    const fromUrl = params.get('invoice');
    if (isNew && fromUrl && !textOf(form.getFieldValue(F.salesInvoice))) {
      form.setFieldValue(F.salesInvoice, fromUrl);
      onChange();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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

  // 原發票的客戶與稅別（伺服器存檔時也會帶入；先帶入讓金額預覽可以計算稅額）
  const invoice = useRecordById(SALES_INVOICE, invoiceId);
  useEffect(() => {
    const d = invoice.data?.data;
    if (!d) {
      return;
    }
    if (
      form.getFieldValue(F.customer) !== d.customer_id ||
      form.getFieldValue(F.taxType) !== d.tax_type
    ) {
      form.setFieldsValue({ [F.customer]: d.customer_id, [F.taxType]: d.tax_type });
      onChange();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [invoice.data]);

  return <AllowanceAvailable invoiceId={invoiceId} />;
}

/** 原發票的可折讓金額（發票總計 − 已開立折讓的合計）。 */
function AllowanceAvailable({ invoiceId }: { invoiceId: string | undefined }) {
  const invoice = useRecordById(SALES_INVOICE, invoiceId);
  const posted = usePostedAllowances(invoiceId);
  if (!invoice.data || !posted.data) {
    return null;
  }
  const status = documentStatus(invoice.data);
  if (status !== 'posted') {
    return (
      <Alert
        showIcon
        type="warning"
        style={{ marginBottom: 16 }}
        message={status === 'void' ? '原發票已作廢，不能開立折讓' : '原發票尚未開立，不能開立折讓'}
      />
    );
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
      description={`發票 ${String(invoice.data.data.invoice_no ?? '')} 總計 ${money(
        String(invoice.data.data.total ?? '0'),
      )}，已折讓 ${money(used)}`}
    />
  );
}

/** 出貨單（已過帳、已作廢）上的發票號碼：關聯的銷項發票，含已作廢。 */
export function InvoiceLinksNote({ entityKey, record }: { entityKey: string; record: DataRecord }) {
  const links = useInvoiceLinks(entityKey, record.id);
  const items = links.data ?? [];
  return (
    <Typography.Paragraph>
      <Space wrap>
        <span>
          <LinkOutlined /> 發票：
        </span>
        {links.isLoading ? (
          <Typography.Text type="secondary">讀取中</Typography.Text>
        ) : items.length === 0 ? (
          <Typography.Text type="secondary">尚未開立發票</Typography.Text>
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
