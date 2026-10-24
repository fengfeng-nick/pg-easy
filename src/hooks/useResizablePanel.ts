import { useCallback } from "react";

/** Drag handler for the sidebar splitter. Width updates live via `onResize`;
 *  the pointer is captured on `window` so a fast drag never loses the handle. */
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
