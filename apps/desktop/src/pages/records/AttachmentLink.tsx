import { PaperClipOutlined } from '@ant-design/icons';
import { App, Button, ConfigProvider } from 'antd';
import { useState } from 'react';

import { downloadAttachment } from '../../api/attachments';
import type { Attachment } from '../../api/attachments';
import { errorMessage } from '../../api/client';

/**
 * 附件的檔名，點擊後選擇存檔位置並下載。
 *
 * 唯讀者也可以下載：不受外層表單的停用狀態影響。點擊不會傳到外層（列表的列點擊會開啟資料）。
 */
export function AttachmentLink({
  attachment,
}: {
  attachment: Pick<Attachment, 'id' | 'file_name'>;
}) {
  const { message } = App.useApp();
  const [downloading, setDownloading] = useState(false);

  const download = async () => {
    setDownloading(true);
    try {
      if (await downloadAttachment(attachment.id, attachment.file_name)) {
        message.success(`已儲存「${attachment.file_name}」`);
      }
    } catch (err) {
      message.error(`下載失敗：${errorMessage(err)}`);
    } finally {
      setDownloading(false);
    }
  };

  return (
    <ConfigProvider componentDisabled={false}>
      <Button
        type="link"
        size="small"
        icon={<PaperClipOutlined />}
        loading={downloading}
        title={`下載 ${attachment.file_name}`}
        style={{ paddingInline: 0, maxWidth: '100%' }}
        onClick={(e) => {
          e.stopPropagation();
          void download();
        }}
      >
        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{attachment.file_name}</span>
      </Button>
    </ConfigProvider>
  );
}
