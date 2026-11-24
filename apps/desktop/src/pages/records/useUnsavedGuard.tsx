import { App, Modal } from 'antd';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useBlocker } from 'react-router';

/** 有未存檔修改時，確認視窗的文字。 */
export const UNSAVED_TITLE = '有未存檔的修改';
export const UNSAVED_CONTENT = '離開後，目前的修改不會儲存。確定要離開？';

/**
 * 未存檔修改的離開提醒（表單頁與分割檢視共用；同一個畫面只能有一個）。
 *
 * - 換到其他頁（路徑改變）時以確認視窗攔下（`useBlocker`）；只改變 query string（例如分割檢視的
 *   `?selected=`）不攔，由 {@link UnsavedGuard.confirmDiscard} 處理
 * - 關閉視窗或重新整理時以瀏覽器的提示攔下
 * - `setDirty` 要同步更新：存檔成功後會立刻導覽，不能等 state 更新
 */
export interface UnsavedGuard {
  setDirty: (dirty: boolean) => void;
  /** 目前是否有未存檔的修改（同步讀取）。 */
  isDirty: () => boolean;
  /** 沒有未存檔的修改時直接執行 `proceed`；有的話先確認，選「離開」才執行（並清除未存檔狀態）。 */
  confirmDiscard: (proceed: () => void) => void;
  /** 換頁時的確認視窗，放在畫面中。 */
  blockerModal: ReactNode;
}

export function useUnsavedGuard(): UnsavedGuard {
  const { modal } = App.useApp();
  const dirtyRef = useRef(false);
  const [dirty, setDirtyState] = useState(false);
  const setDirty = useCallback((value: boolean) => {
    dirtyRef.current = value;
    setDirtyState(value);
  }, []);

  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirtyRef.current && currentLocation.pathname !== nextLocation.pathname,
  );

  // 關閉視窗或重新整理時也提醒
  useEffect(() => {
    if (!dirty) {
      return;
    }
    const onBeforeUnload = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, [dirty]);

  const confirmDiscard = useCallback(
    (proceed: () => void) => {
      if (!dirtyRef.current) {
        proceed();
        return;
      }
      modal.confirm({
        title: UNSAVED_TITLE,
        content: UNSAVED_CONTENT,
        okText: '離開',
        okButtonProps: { danger: true },
        cancelText: '留下',
        onOk: () => {
          setDirty(false);
          proceed();
        },
      });
    },
    [modal, setDirty],
  );

  const blockerModal = (
    <Modal
      open={blocker.state === 'blocked'}
      title={UNSAVED_TITLE}
      okText="離開"
      okButtonProps={{ danger: true }}
      cancelText="留下"
      onOk={() => blocker.proceed?.()}
      onCancel={() => blocker.reset?.()}
    >
      {UNSAVED_CONTENT}
    </Modal>
  );

  const isDirty = useCallback(() => dirtyRef.current, []);

  return { setDirty, isDirty, confirmDiscard, blockerModal };
}
