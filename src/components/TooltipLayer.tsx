import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

const SHOW_DELAY_MS = 350;
const VIEWPORT_GAP = 10;
const TOOLTIP_OFFSET = 8;

interface TooltipState {
  text: string;
  x: number;
  y: number;
  placement: "top" | "bottom";
}

function tooltipTarget(target: EventTarget | null): HTMLElement | null {
  return target instanceof Element
    ? (target.closest("[data-tooltip]") as HTMLElement | null)
    : null;
}

export function TooltipLayer() {
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    const clearTimer = () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };

    const hide = () => {
      clearTimer();
      setTooltip(null);
    };

    const show = (element: HTMLElement, delayed: boolean) => {
      const text = element.dataset.tooltip?.trim();
      if (!text) return;

      clearTimer();
      const reveal = () => {
        const rect = element.getBoundingClientRect();
        const placement = rect.bottom + 52 < window.innerHeight ? "bottom" : "top";
        const maxTooltipHalfWidth = Math.min(144, (window.innerWidth - VIEWPORT_GAP * 2) / 2);
        setTooltip({
          text,
          x: Math.min(
            window.innerWidth - VIEWPORT_GAP - maxTooltipHalfWidth,
            Math.max(VIEWPORT_GAP + maxTooltipHalfWidth, rect.left + rect.width / 2),
          ),
          y: placement === "bottom" ? rect.bottom + TOOLTIP_OFFSET : rect.top - TOOLTIP_OFFSET,
          placement,
        });
      };

      if (delayed) timerRef.current = window.setTimeout(reveal, SHOW_DELAY_MS);
      else reveal();
    };

    const onMouseOver = (event: MouseEvent) => {
      const element = tooltipTarget(event.target);
      if (!element || element.contains(event.relatedTarget as Node | null)) return;
      show(element, true);
    };
    const onMouseOut = (event: MouseEvent) => {
      const element = tooltipTarget(event.target);
      if (!element || element.contains(event.relatedTarget as Node | null)) return;
      hide();
    };
    const onFocusIn = (event: FocusEvent) => {
      const element = tooltipTarget(event.target);
      if (element) show(element, false);
    };
    const onFocusOut = (event: FocusEvent) => {
      if (tooltipTarget(event.target)) hide();
    };

    document.addEventListener("mouseover", onMouseOver);
    document.addEventListener("mouseout", onMouseOut);
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    window.addEventListener("blur", hide);
    window.addEventListener("resize", hide);
    window.addEventListener("scroll", hide, true);

    return () => {
      clearTimer();
      document.removeEventListener("mouseover", onMouseOver);
      document.removeEventListener("mouseout", onMouseOut);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
      window.removeEventListener("blur", hide);
      window.removeEventListener("resize", hide);
      window.removeEventListener("scroll", hide, true);
    };
  }, []);

  if (!tooltip) return null;

  return createPortal(
    <div
      className={`app-tooltip app-tooltip--${tooltip.placement}`}
      role="tooltip"
      style={{ left: tooltip.x, top: tooltip.y }}
    >
      {tooltip.text}
    </div>,
    document.body,
  );
}
