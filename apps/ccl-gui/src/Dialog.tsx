import { useEffect, useRef, type ReactNode } from 'react'

export function ModalFrame({ title, close, closeLabel, children, role = 'dialog', descriptionId }: { title: string; close: () => void; closeLabel: string; children: ReactNode; role?: 'dialog' | 'alertdialog'; descriptionId?: string }) {
  const panel = useRef<HTMLElement>(null)
  const closeRef = useRef(close)
  closeRef.current = close
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const element = panel.current
    const first = element?.querySelector<HTMLElement>('input:not([disabled]), select:not([disabled])') ?? element?.querySelector<HTMLElement>('button:not([disabled])')
    first?.focus()
    return () => { if (previous?.isConnected) previous.focus() }
  }, [])
  return <div className="modal-backdrop" role="presentation"><section ref={panel} className="modal-card" role={role} aria-modal="true" aria-labelledby="modal-title" aria-describedby={descriptionId} onKeyDown={event => {
    if (event.key === 'Escape') { event.preventDefault(); closeRef.current(); return }
    if (event.key !== 'Tab') return
    const focusable = [...(panel.current?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex="0"]') ?? [])].filter(element => element.offsetParent !== null)
    const first = focusable[0], last = focusable.at(-1)
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus() }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus() }
  }}><header><h2 id="modal-title">{title}</h2><button type="button" className="icon-button" onClick={close} aria-label={closeLabel}>×</button></header>{children}</section></div>
}

