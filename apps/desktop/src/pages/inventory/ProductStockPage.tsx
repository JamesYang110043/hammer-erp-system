import { ArrowLeftOutlined, ReloadOutlined } from '@ant-design/icons';
import { Alert, Button, Card, Checkbox, Select, Space, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';

import { NumberText } from '../../components/NumberText';
import { PageHeader } from '../../components/PageHeader';
import { SectionTitle } from '../../components/SectionTitle';
import { errorMessage } from '../../api/client';
import { useInventoryTransactions, useProductLots } from '../../api/inventory';
import { useRecord } from '../../api/records';
import { useEntities } from '../../api/schema';
import type {
  EntityDefinition,
  InventoryLot_Serialize as InventoryLot,
  InventoryTransaction_Serialize as InventoryTransaction,
  TxnType,
} from '../../bindings';
import { recordPath } from '../../modules';
import { formatQty } from '../../records/documents';
import {
  STOCK_PATH,
  TXN_TYPE_LABELS,
  documentKeyOfLine,
  formatQtyChange,
  formatUnitCost,
} from '../../records/inventory';
import { formatTaipei } from '../../utils/time';

const PAGE_SIZE = 50;
const PRODUCT_ENTITY = 'product';

/** 來源單據的連結；不知道資料類別時只顯示單號。 */
function DocumentLink({
  entityKey,
  id,
  recordNo,
}: {
  entityKey?: string;
  id: string;
  recordNo: string;
}) {
  return entityKey ? <Link to={recordPath(entityKey, id)}>{recordNo}</Link> : <>{recordNo}</>;
}

function lotColumns(
  entities: readonly EntityDefinition[] | undefined,
): TableColumnsType<InventoryLot> {
  return [
    { title: '批號', dataIndex: 'lot_no' },
    { title: '入庫日期', dataIndex: 'received_date', width: 120 },
    {
      title: '來源單據',
      key: 'source',
      render: (_, lot) => (
        <DocumentLink
          entityKey={documentKeyOfLine(entities, lot.source_type)}
          id={lot.source_doc_id}
          recordNo={lot.source_record_no}
        />
      ),
    },
    {
      title: '入庫數量',
      dataIndex: 'qty_in',
      align: 'right',
      render: (v: string) => formatQty(v),
    },
    {
      title: '剩餘數量',
      dataIndex: 'qty_remaining',
      align: 'right',
      render: (v: string) => <Typography.Text strong>{formatQty(v)}</Typography.Text>,
    },
    {
      title: '單位成本',
      dataIndex: 'unit_cost_twd',
      align: 'right',
      render: (v: string) => <NumberText>{formatUnitCost(v)}</NumberText>,
    },
  ];
}

const txnColumns: TableColumnsType<InventoryTransaction> = [
  { title: '日期', dataIndex: 'txn_date', width: 110 },
  {
    title: '類型',
    dataIndex: 'txn_type',
    width: 100,
    render: (t: TxnType) => (
      <Tag color={t === 'void_reversal' ? 'red' : undefined}>{TXN_TYPE_LABELS[t]}</Tag>
    ),
  },
  { title: '批號', dataIndex: 'lot_no' },
  {
    title: '數量',
    dataIndex: 'qty_change',
    align: 'right',
    render: (v: string) => (
      <Typography.Text type={v.startsWith('-') ? 'danger' : 'success'}>
        {formatQtyChange(v)}
      </Typography.Text>
    ),
  },
  {
    title: '單位成本',
    dataIndex: 'unit_cost_twd',
    align: 'right',
    render: (v: string) => <NumberText>{formatUnitCost(v)}</NumberText>,
  },
  {
    title: '來源單據',
    key: 'ref',
    render: (_, t) => (
      <DocumentLink entityKey={t.ref_doc_type} id={t.ref_doc_id} recordNo={t.ref_record_no} />
    ),
  },
  {
    title: '經手人',
    dataIndex: 'created_by_name',
    render: (v: string | null) => v ?? <Typography.Text type="secondary">—</Typography.Text>,
  },
  {
    title: '異動時間',
    dataIndex: 'txn_at',
    width: 170,
    render: (v: string) => formatTaipei(v),
  },
];

/**
 * 商品的庫存明細（`/stock/:productId`，需要 `record.read`）：批號（先進先出順序）與異動流水帳。
 *
 * 點批號只看該批號的異動；批號、異動的單號連到來源單據。
 */
export function ProductStockPage() {
  const { productId = '' } = useParams();
  const navigate = useNavigate();
  const product = useRecord(PRODUCT_ENTITY, productId);
  const entities = useEntities();

  const [includeEmpty, setIncludeEmpty] = useState(false);
  const [lotPage, setLotPage] = useState(1);
  const [lot, setLot] = useState<InventoryLot>();
  const [txnType, setTxnType] = useState<TxnType>();
  const [txnPage, setTxnPage] = useState(1);

  const lots = useProductLots(productId, {
    include_empty: includeEmpty ? 'true' : null,
    page: lotPage,
    page_size: PAGE_SIZE,
  });
  const txns = useInventoryTransactions({
    product_id: productId,
    lot_id: lot?.id ?? null,
    txn_type: txnType ?? null,
    page: txnPage,
    page_size: PAGE_SIZE,
  });

  const code = product.data?.data.code;
  const name = product.data?.data.name;
  const title =
    typeof code === 'string' && typeof name === 'string'
      ? `${code} ${name}`
      : (product.data?.title ?? '商品庫存');
  const error = product.error ?? lots.error;

  return (
    <div className="page">
      <PageHeader
        title={title}
        actions={
          <Space>
            <Button
              icon={<ReloadOutlined />}
              loading={lots.isFetching || txns.isFetching}
              onClick={() => {
                void lots.refetch();
                void txns.refetch();
              }}
            >
              重新整理
            </Button>
            <Button icon={<ArrowLeftOutlined />} onClick={() => navigate(`/${STOCK_PATH}`)}>
              返回庫存查詢
            </Button>
          </Space>
        }
      />
      <Card>
        {error && (
          <Alert showIcon type="error" message={errorMessage(error)} style={{ marginBottom: 16 }} />
        )}
        <SectionTitle
          extra={
            <Checkbox
              checked={includeEmpty}
              onChange={(e) => {
                setIncludeEmpty(e.target.checked);
                setLotPage(1);
              }}
            >
              含已用完的批號
            </Checkbox>
          }
        >
          批號
        </SectionTitle>
        <Table<InventoryLot>
          aria-label="批號"
          rowKey="id"
          size="small"
          columns={lotColumns(entities.data)}
          dataSource={lots.data?.items}
          loading={lots.isFetching}
          scroll={{ x: 'max-content' }}
          locale={{ emptyText: '沒有批號' }}
          rowSelection={{
            type: 'radio',
            selectedRowKeys: lot ? [lot.id] : [],
            onChange: (_, rows) => {
              setLot(rows[0]);
              setTxnPage(1);
            },
          }}
          onRow={(row) => ({
            onClick: () => {
              setLot(row);
              setTxnPage(1);
            },
            style: { cursor: 'pointer' },
          })}
          pagination={{
            current: lots.data?.page ?? lotPage,
            pageSize: PAGE_SIZE,
            total: lots.data?.total ?? 0,
            hideOnSinglePage: true,
            onChange: setLotPage,
          }}
        />
      </Card>
      <Card
        title={
          <Space>
            異動流水帳
            {lot && (
              <Tag
                closable
                onClose={() => {
                  setLot(undefined);
                  setTxnPage(1);
                }}
              >
                批號 {lot.lot_no}
              </Tag>
            )}
          </Space>
        }
        extra={
          <Select<TxnType>
            aria-label="異動類型"
            allowClear
            placeholder="全部類型"
            style={{ width: 140 }}
            value={txnType}
            onChange={(v) => {
              setTxnType(v);
              setTxnPage(1);
            }}
            options={Object.entries(TXN_TYPE_LABELS).map(([value, label]) => ({
              value: value as TxnType,
              label,
            }))}
          />
        }
      >
        {txns.isError && (
          <Alert
            showIcon
            type="error"
            message={errorMessage(txns.error)}
            style={{ marginBottom: 16 }}
          />
        )}
        <Table<InventoryTransaction>
          aria-label="異動流水帳"
          rowKey="id"
          size="small"
          columns={txnColumns}
          dataSource={txns.data?.items}
          loading={txns.isFetching}
          scroll={{ x: 'max-content' }}
          locale={{ emptyText: '沒有異動' }}
          pagination={{
            current: txns.data?.page ?? txnPage,
            pageSize: PAGE_SIZE,
            total: txns.data?.total ?? 0,
            showTotal: (total) => `共 ${total} 筆`,
            onChange: setTxnPage,
          }}
        />
      </Card>
    </div>
  );
}
