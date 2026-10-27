import { Children, Fragment, isValidElement, useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { filterOptions, menuPosition, nextOption, type SelectOption } from './select-model'
import './Select.css'

interface SelectProps {
  value: string | number
  onValueChange: (value: string) => void
  children: ReactNode
  disabled?: boolean
  required?: boolean
  id?: string
  'aria-label'?: string
}

function textOf(node: ReactNode): string {
  return Children.toArray(node).map(child => isValidElement<{ children?: ReactNode }>(child) ? textOf(child.props.children) : String(child)).join('')
}

function optionsOf(children: ReactNode): SelectOption[] {
  const options: SelectOption[] = []
  Children.forEach(children, child => {
    if (!isValidElement<{ children?: ReactNode; value?: string | number; disabled?: boolean }>(child)) return
    if (child.type === Fragment) options.push(...optionsOf(child.props.children))
    else if (child.type === 'option') {
      const label = textOf(child.props.children)
      options.push({ value: String(child.props.value ?? label), label, disabled: !!child.props.disabled })
    }
  })
  return options
}

/** Shared single-choice control. Option children are data, never native popup UI. */
export function Select({ value, onValueChange, children, disabled = false, required, id, 'aria-label': ariaLabel }: SelectProps) {
  const uid = useId(), listId = `${uid}-list`, triggerId = id ?? `${uid}-trigger`
  const trigger = useRef<HTMLButtonElement>(null), popup = useRef<HTMLDivElement>(null), search = useRef<HTMLInputElement>(null)
  const [open, setOpen] = useState(false), [query, setQuery] = useState(''), [active, setActive] = useState<string | null>(null)
  const [position, setPosition] = useState<CSSProperties>({}), [host, setHost] = useState<HTMLElement | null>(null)
  const options = optionsOf(children), selected = options.find(option => option.value === String(value))
  const optionKey = JSON.stringify(options)
  const visible = filterOptions(options, query), searchable = options.length > 8
  const zh = document.documentElement.lang.toLowerCase().startsWith('zh')
  const activeValue = visible.some(option => option.value === active && !option.disabled) ? active : nextOption(visible, null, 1)
  const activeIndex = visible.findIndex(option => option.value === activeValue && !option.disabled)
  const activeId = activeIndex < 0 ? undefined : `${uid}-option-${activeIndex}`
  const typeahead = useRef({ text: '', at: 0 })

  function close(restore = true) {
    setOpen(false)
    if (restore && trigger.current?.isConnected) trigger.current.focus()
  }
  function expand(direction: 1 | -1 = 1) {
    if (disabled) return
    setQuery('')
    setActive(selected && !selected.disabled ? selected.value : nextOption(options, null, direction))
    // Keep popup focus within the owning modal's focus trap.
    setHost(trigger.current?.closest<HTMLElement>('.modal-card') ?? document.body)
    setOpen(true)
  }
  function choose(option: SelectOption) {
    if (option.disabled) return
    close()
    if (option.value !== String(value)) onValueChange(option.value)
  }
  function keyboard(event: KeyboardEvent<HTMLElement>) {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return
    if (!open) {
      if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(event.key)) {
        event.preventDefault(); event.stopPropagation(); expand(event.key === 'ArrowUp' ? -1 : 1)
      }
      return
    }
    // Escape closes this menu before a surrounding dialog handles it.
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); return }
    if (event.key === 'Tab') {
      event.preventDefault(); event.stopPropagation()
      const scope = trigger.current?.closest('.modal-card') ?? document
      const controls = [...scope.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [tabindex="0"], summary, a[href]')]
        .filter(element => element.offsetParent !== null && element.tabIndex >= 0 && !popup.current?.contains(element))
      const index = controls.indexOf(trigger.current!)
      const next = controls[index + (event.shiftKey ? -1 : 1)] ?? (event.shiftKey ? controls.at(-1) : controls[0])
      close(false); next?.focus(); return
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault(); event.stopPropagation(); setActive(nextOption(visible, activeValue, event.key === 'ArrowDown' ? 1 : -1)); return
    }
    if (event.key === 'Enter' || (event.key === ' ' && event.currentTarget === trigger.current)) {
      event.preventDefault(); event.stopPropagation()
      if (activeIndex >= 0) choose(visible[activeIndex])
      return
    }
    if (event.currentTarget === search.current) return
    if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault(); setActive(nextOption(visible, null, event.key === 'Home' ? 1 : -1)); return
    }
    if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault()
      const now = Date.now(), previous = now - typeahead.current.at < 700 ? typeahead.current.text : ''
      const text = previous + event.key
      typeahead.current = { text, at: now }
      const match = filterOptions(options, text).find(option => !option.disabled)
      if (match) setActive(match.value)
    }
  }

  useEffect(() => { if (disabled) setOpen(false) }, [disabled])
  useLayoutEffect(() => {
    if (!open) return
    const update = () => {
      if (!trigger.current) return
      const rect = trigger.current.getBoundingClientRect()
      if (rect.bottom < 0 || rect.top > window.innerHeight || rect.right < 0 || rect.left > window.innerWidth) { setOpen(false); return }
      const labels = popup.current?.querySelectorAll<HTMLElement>('.ccl-select-option > span:first-child') ?? []
      const contentWidth = Math.min(420, Math.max(0, ...[...labels].map(label => label.scrollWidth + 56)))
      const listHeight = popup.current?.querySelector<HTMLElement>('.ccl-select-list')?.scrollHeight ?? 0
      const searchHeight = popup.current?.querySelector<HTMLElement>('.ccl-select-search')?.offsetHeight ?? 0
      const emptyHeight = popup.current?.querySelector<HTMLElement>('.ccl-select-empty')?.offsetHeight ?? 0
      const desiredHeight = Math.min(320, listHeight + searchHeight + emptyHeight + 12)
      const next = menuPosition({ top: rect.top, bottom: rect.bottom, left: rect.left, width: Math.max(rect.width, contentWidth) }, { width: window.innerWidth, height: window.innerHeight }, desiredHeight)
      setPosition(previous => JSON.stringify(previous) === JSON.stringify(next) ? previous : next)
    }
    update()
    window.addEventListener('resize', update)
    document.addEventListener('scroll', update, true)
    const observer = new ResizeObserver(update)
    if (trigger.current) observer.observe(trigger.current)
    return () => { window.removeEventListener('resize', update); document.removeEventListener('scroll', update, true); observer.disconnect() }
  }, [open, optionKey])
  useEffect(() => {
    if (!open) return
    if (searchable) search.current?.focus()
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !trigger.current?.contains(event.target) && !popup.current?.contains(event.target)) close(false)
    }
    const blur = () => close(false)
    document.addEventListener('pointerdown', outside, true); window.addEventListener('blur', blur)
    return () => { document.removeEventListener('pointerdown', outside, true); window.removeEventListener('blur', blur) }
    // Opening alone moves focus; typing and changing options must not steal it.
  }, [open, searchable])
  useLayoutEffect(() => {
    // Scroll only the option list, never the page or the owning dialog.
    const item = open && activeId ? document.getElementById(activeId) : null
    const list = item?.parentElement
    if (!item || !list) return
    if (item.offsetTop < list.scrollTop) list.scrollTop = item.offsetTop
    else if (item.offsetTop + item.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTop = item.offsetTop + item.offsetHeight - list.clientHeight
  }, [open, activeId, position])

  return <span className="ccl-select">
    <button ref={trigger} id={triggerId} type="button" className="ccl-select-trigger" role="combobox" aria-label={ariaLabel}
      aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? listId : undefined} aria-activedescendant={open && !searchable ? activeId : undefined}
      aria-required={required || undefined} disabled={disabled} onClick={() => open ? close() : expand()} onKeyDown={keyboard}>
      <span className="ccl-select-value" title={selected?.label}>{selected?.label ?? (zh ? '请选择' : 'Choose an option')}</span>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 6 4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/></svg>
    </button>
    {required && <input className="ccl-select-validation" aria-hidden="true" tabIndex={-1} required disabled={disabled}
      value={selected?.value ?? ''} onChange={() => {}} onInvalid={event => { event.preventDefault(); trigger.current?.focus(); expand() }}/>}
    {open && host && createPortal(<div ref={popup} className="ccl-select-popup" style={position}>
      {searchable && <div className="ccl-select-search"><input ref={search} type="search" aria-label={zh ? '搜索选项' : 'Search options'} placeholder={zh ? '搜索…' : 'Search…'}
        aria-controls={listId} aria-activedescendant={activeId} value={query} onKeyDown={keyboard} onChange={event => {
          const text = event.target.value; setQuery(text); setActive(nextOption(filterOptions(options, text), null, 1))
        }}/></div>}
      <div id={listId} role="listbox" aria-labelledby={triggerId} className="ccl-select-list">
        {visible.map((option, index) => <div key={option.value} id={`${uid}-option-${index}`} role="option" aria-selected={option.value === String(value)} aria-disabled={option.disabled || undefined}
          className={`ccl-select-option${option.value === activeValue ? ' is-active' : ''}`} title={option.label}
          onPointerMove={() => { if (!option.disabled) setActive(option.value) }} onMouseDown={event => event.preventDefault()} onClick={event => { event.preventDefault(); event.stopPropagation(); choose(option) }}>
          <span>{option.label}</span><span className="ccl-select-check" aria-hidden="true">{option.value === String(value) ? '✓' : ''}</span>
        </div>)}
      </div>
      {!visible.length && <div className="ccl-select-empty" role="status">{zh ? '没有匹配的选项' : 'No matching options'}</div>}
    </div>, host)}
  </span>
}
