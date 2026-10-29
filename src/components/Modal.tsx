import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode, type RefObject } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

interface Props {
  title: string;
  onClose: () => void;
  /** false: "Schließen" and Esc are blocked (e.g. while a download runs). */
  canClose?: boolean;
  closeTitle?: string;
  /** Focus target when the element focused before opening is gone. */
  returnFocus?: RefObject<HTMLElement | null>;
  className?: string;
  children: ReactNode;
}

/**
 * In-app modal dialog: role="dialog", aria-modal, focus trap (Tab/Shift+Tab
 * stay inside), Esc and „Schließen“ close it, focus returns to the element
 * that opened it (or `returnFocus`).
 */
export function Modal({ title, onClose, canClose = true, closeTitle, returnFocus, className, children }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const closeRef = useRef({ onClose, canClose });
  closeRef.current = { onClose, canClose };

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = ref.current;
    // Focus the dialog itself: the title is announced, Tab goes to the first control.
    dialog?.focus();
    return () => {
      const target = opener && opener.isConnected ? opener : returnFocus?.current;
      target?.focus();
    };
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (closeRef.current.canClose) closeRef.current.onClose();
      return;
    }
    if (e.key !== "Tab" || !ref.current) return;
    const items = Array.from(ref.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => el.offsetParent !== null || el === document.activeElement,
    );
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const firstEl = items[0];
    const lastEl = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === firstEl || active === ref.current)) {
      e.preventDefault();
      lastEl.focus();
    } else if (!e.shiftKey && (active === lastEl || !ref.current.contains(active))) {
      e.preventDefault();
      firstEl.focus();
    }
  };

  return (
    <div className="modal-backdrop">
      <div
        ref={ref}
        className={className ? `modal ${className}` : "modal"}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="modal-header">
          <h2 id={titleId}>{title}</h2>
          <button onClick={onClose} disabled={!canClose} title={canClose ? undefined : closeTitle}>
            Schließen
          </button>
        </div>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}
