import { useEffect, useRef, useState } from 'react'

/** 框选起点超过该像素阈值才进入"拖拽框选"模式，避免误把普通点击识别成框选。 */
const DRAG_THRESHOLD = 5

export interface RectSelectionState {
  /** 是否处于框选过程（鼠标拖动距离已经超过阈值） */
  active: boolean
  /** 框选矩形（fixed 定位坐标） */
  rect: { left: number; top: number; width: number; height: number } | null
}

export interface UseRectSelectOptions {
  containerRef: React.RefObject<HTMLElement | null>
  /** 命中元素的 CSS 选择器；要求该元素带有 data-{idAttribute}，存放 number id */
  itemSelector: string
  /** 数据属性名（不带 `data-` 前缀），从命中元素读取 id */
  idAttribute: string
  /** 框选完成时的回调，参数是命中元素的 id 集合 + 是否按住了 Ctrl/Meta */
  onSelectionChange: (ids: Set<number>, additive: boolean) => void
  enabled?: boolean
}

/**
 * 在指定容器内启用鼠标左键拖拽框选。
 *
 * 设计要点：
 * - 仅在按下点不是按钮 / 输入框 / 下拉等交互元素时才启动，避免误触。
 * - 按住 Ctrl/Meta 框选时附加到既有选区，否则替换。
 * - 拖动距离不足阈值时降级为普通点击，由原有 onClick 接管。
 * - 用 fixed 定位虚线矩形显示选区，避免污染父级布局。
 */
export function useRectSelect(options: UseRectSelectOptions): RectSelectionState {
  const { containerRef, itemSelector, idAttribute, onSelectionChange, enabled = true } = options
  const [state, setState] = useState<RectSelectionState>({ active: false, rect: null })

  const startRef = useRef<{ x: number; y: number; additive: boolean } | null>(null)
  const activeRef = useRef(false)

  useEffect(() => {
    if (!enabled) return
    const el = containerRef.current
    if (!el) return

    const isInteractive = (target: EventTarget | null): boolean => {
      if (!(target instanceof HTMLElement)) return false
      return Boolean(
        target.closest(
          'button, a, input, select, textarea, label, [role="checkbox"], [role="menuitem"], [data-no-rect-select]'
        )
      )
    }

    const dataAttr = `data-${idAttribute}`

    const onMouseDown = (e: MouseEvent) => {
      if (e.button !== 0) return
      if (isInteractive(e.target)) return
      startRef.current = { x: e.clientX, y: e.clientY, additive: e.ctrlKey || e.metaKey }
      activeRef.current = false
    }

    const onMouseMove = (e: MouseEvent) => {
      const start = startRef.current
      if (!start) return
      const dx = e.clientX - start.x
      const dy = e.clientY - start.y

      if (!activeRef.current) {
        if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) return
        activeRef.current = true
        document.body.style.userSelect = 'none'
      }

      const left = Math.min(e.clientX, start.x)
      const top = Math.min(e.clientY, start.y)
      const width = Math.abs(dx)
      const height = Math.abs(dy)
      setState({ active: true, rect: { left, top, width, height } })
    }

    const onMouseUp = (e: MouseEvent) => {
      const start = startRef.current
      startRef.current = null
      if (!start) return
      if (!activeRef.current) return // 普通点击不改变选区
      activeRef.current = false
      document.body.style.userSelect = ''

      const left = Math.min(e.clientX, start.x)
      const top = Math.min(e.clientY, start.y)
      const right = Math.max(e.clientX, start.x)
      const bottom = Math.max(e.clientY, start.y)

      const items = el.querySelectorAll<HTMLElement>(itemSelector)
      const hits = new Set<number>()
      items.forEach((item) => {
        const r = item.getBoundingClientRect()
        const overlap = r.left < right && r.right > left && r.top < bottom && r.bottom > top
        if (!overlap) return
        const raw = item.getAttribute(dataAttr)
        const id = raw ? Number(raw) : NaN
        if (Number.isFinite(id)) hits.add(id)
      })

      onSelectionChange(hits, start.additive)
      setState({ active: false, rect: null })
    }

    el.addEventListener('mousedown', onMouseDown)
    window.addEventListener('mousemove', onMouseMove)
    window.addEventListener('mouseup', onMouseUp)

    return () => {
      el.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('mousemove', onMouseMove)
      window.removeEventListener('mouseup', onMouseUp)
      document.body.style.userSelect = ''
    }
  }, [containerRef, itemSelector, idAttribute, onSelectionChange, enabled])

  return state
}
