import { useCallback } from "react";

/** 侧边栏分隔条的拖拽处理。宽度通过 `onResize` 实时更新；指针在
 *  `window` 上捕获，快速拖动也不会丢掉手柄。 */
export function useResizablePanel(onResize: (width: number) => void) {
  const startDrag = useCallback(
    (event: React.PointerEvent) => {
      event.preventDefault();
      const startX = event.clientX;
      const startWidth = (event.currentTarget.previousElementSibling as HTMLElement)?.offsetWidth ?? 252;

      const onMove = (move: PointerEvent) => {
        onResize(Math.round(startWidth + move.clientX - startX));
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [onResize],
  );

  return { startDrag };
}
