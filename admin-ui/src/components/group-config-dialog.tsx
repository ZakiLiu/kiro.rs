import { useState, useEffect } from 'react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { useGroupConfig, useUpdateGroupConfig } from '@/hooks/use-groups'
import { toast } from 'sonner'
import { extractErrorMessage } from '@/lib/utils'
import type { GroupConfigOverrides, GroupCompressionOverrides } from '@/api/groups'

interface GroupConfigDialogProps {
  groupName: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function GroupConfigDialog({ groupName, open, onOpenChange }: GroupConfigDialogProps) {
  const { data, isLoading } = useGroupConfig(open ? groupName : null)
  const { mutateAsync, isPending } = useUpdateGroupConfig()

  const [credentialRpm, setCredentialRpm] = useState('')
  const [credentialDailyMaxRequests, setCredentialDailyMaxRequests] = useState('')
  const [defaultEndpoint, setDefaultEndpoint] = useState('')
  const [promptCacheTtlSeconds, setPromptCacheTtlSeconds] = useState('')

  const [cEnabled, setCEnabled] = useState<boolean | null>(null)
  const [cWhitespace, setCWhitespace] = useState<boolean | null>(null)
  const [cThinkingStrategy, setCThinkingStrategy] = useState('')
  const [cToolResultMaxChars, setCToolResultMaxChars] = useState('')
  const [cToolResultHeadLines, setCToolResultHeadLines] = useState('')
  const [cToolResultTailLines, setCToolResultTailLines] = useState('')
  const [cToolUseInputMaxChars, setCToolUseInputMaxChars] = useState('')
  const [cToolDescMaxChars, setCToolDescMaxChars] = useState('')
  const [cMaxHistoryTurns, setCMaxHistoryTurns] = useState('')
  const [cMaxHistoryChars, setCMaxHistoryChars] = useState('')
  const [cMaxRequestBodyBytes, setCMaxRequestBodyBytes] = useState('')
  const [cMaxInputTokens, setCMaxInputTokens] = useState('')

  useEffect(() => {
    if (!open || !data) return
    const o = data.overrides
    setCredentialRpm(o.credentialRpm?.toString() ?? '')
    setCredentialDailyMaxRequests(o.credentialDailyMaxRequests?.toString() ?? '')
    setDefaultEndpoint(o.defaultEndpoint ?? '')
    setPromptCacheTtlSeconds(o.promptCacheTtlSeconds?.toString() ?? '')
    const c = o.compression
    setCEnabled(c?.enabled ?? null)
    setCWhitespace(c?.whitespaceCompression ?? null)
    setCThinkingStrategy(c?.thinkingStrategy ?? '')
    setCToolResultMaxChars(c?.toolResultMaxChars?.toString() ?? '')
    setCToolResultHeadLines(c?.toolResultHeadLines?.toString() ?? '')
    setCToolResultTailLines(c?.toolResultTailLines?.toString() ?? '')
    setCToolUseInputMaxChars(c?.toolUseInputMaxChars?.toString() ?? '')
    setCToolDescMaxChars(c?.toolDescriptionMaxChars?.toString() ?? '')
    setCMaxHistoryTurns(c?.maxHistoryTurns?.toString() ?? '')
    setCMaxHistoryChars(c?.maxHistoryChars?.toString() ?? '')
    setCMaxRequestBodyBytes(c?.maxRequestBodyBytes?.toString() ?? '')
    setCMaxInputTokens(c?.maxInputTokens?.toString() ?? '')
  }, [open, data])

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!groupName) return

    const comp: GroupCompressionOverrides = {}
    let hasComp = false
    const setComp = <K extends keyof GroupCompressionOverrides>(k: K, v: GroupCompressionOverrides[K]) => {
      if (v !== null && v !== undefined && v !== '') { comp[k] = v; hasComp = true }
    }
    if (cEnabled !== null) { comp.enabled = cEnabled; hasComp = true }
    if (cWhitespace !== null) { comp.whitespaceCompression = cWhitespace; hasComp = true }
    setComp('thinkingStrategy', cThinkingStrategy || null)
    setComp('toolResultMaxChars', cToolResultMaxChars ? parseInt(cToolResultMaxChars) : null)
    setComp('toolResultHeadLines', cToolResultHeadLines ? parseInt(cToolResultHeadLines) : null)
    setComp('toolResultTailLines', cToolResultTailLines ? parseInt(cToolResultTailLines) : null)
    setComp('toolUseInputMaxChars', cToolUseInputMaxChars ? parseInt(cToolUseInputMaxChars) : null)
    setComp('toolDescriptionMaxChars', cToolDescMaxChars ? parseInt(cToolDescMaxChars) : null)
    setComp('maxHistoryTurns', cMaxHistoryTurns ? parseInt(cMaxHistoryTurns) : null)
    setComp('maxHistoryChars', cMaxHistoryChars ? parseInt(cMaxHistoryChars) : null)
    setComp('maxRequestBodyBytes', cMaxRequestBodyBytes ? parseInt(cMaxRequestBodyBytes) : null)
    setComp('maxInputTokens', cMaxInputTokens ? parseInt(cMaxInputTokens) : null)

    const config: GroupConfigOverrides = {}
    if (credentialRpm.trim()) config.credentialRpm = parseInt(credentialRpm)
    if (credentialDailyMaxRequests.trim()) config.credentialDailyMaxRequests = parseInt(credentialDailyMaxRequests)
    if (defaultEndpoint.trim()) config.defaultEndpoint = defaultEndpoint.trim()
    if (promptCacheTtlSeconds.trim()) config.promptCacheTtlSeconds = parseInt(promptCacheTtlSeconds)
    if (hasComp) config.compression = comp

    try {
      await mutateAsync({ name: groupName, config })
      toast.success(`分组 ${groupName} 配置已保存`)
      onOpenChange(false)
    } catch (err) {
      toast.error(extractErrorMessage(err))
    }
  }

  const resolved = data?.resolved

  const numInput = (
    id: string, label: string, value: string,
    setter: (v: string) => void, fallback?: number | null,
  ) => (
    <div className="space-y-1">
      <label htmlFor={id} className="text-sm font-medium">{label}</label>
      <Input
        id={id} type="number" min={0} value={value}
        onChange={(e) => setter(e.target.value)}
        placeholder={fallback != null ? `继承全局: ${fallback}` : '继承全局'}
        disabled={isPending}
      />
    </div>
  )

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>分组配置：{groupName}</DialogTitle>
          <p className="text-xs text-muted-foreground">留空的字段自动继承全局配置</p>
        </DialogHeader>

        {isLoading ? (
          <div className="py-8 text-center text-muted-foreground">加载中...</div>
        ) : (
          <form onSubmit={handleSubmit} className="space-y-6">
            <div className="space-y-3">
              <h3 className="text-sm font-semibold text-muted-foreground">基本设置</h3>
              {numInput('gcRpm', 'Credential RPM', credentialRpm, setCredentialRpm, resolved?.credentialRpm)}
              {numInput('gcDailyMax', 'Credential Daily Max', credentialDailyMaxRequests, setCredentialDailyMaxRequests, resolved?.credentialDailyMaxRequests)}
              <div className="space-y-1">
                <label htmlFor="gcEndpoint" className="text-sm font-medium">默认 Endpoint</label>
                <select
                  id="gcEndpoint"
                  className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                  value={defaultEndpoint}
                  onChange={(e) => setDefaultEndpoint(e.target.value)}
                  disabled={isPending}
                >
                  <option value="">继承全局 ({resolved?.defaultEndpoint ?? 'ide'})</option>
                  <option value="ide">ide</option>
                  <option value="ide-runtime">ide-runtime</option>
                  <option value="cli">cli</option>
                </select>
              </div>
              <div className="space-y-1">
                <label htmlFor="gcCacheTtl" className="text-sm font-medium">Prompt Cache TTL</label>
                <select
                  id="gcCacheTtl"
                  className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                  value={promptCacheTtlSeconds}
                  onChange={(e) => setPromptCacheTtlSeconds(e.target.value)}
                  disabled={isPending}
                >
                  <option value="">继承全局 ({resolved?.promptCacheTtlSeconds ?? 300}s)</option>
                  <option value="300">5 分钟</option>
                  <option value="3600">1 小时</option>
                  <option value="7200">2 小时</option>
                  <option value="18000">5 小时</option>
                </select>
              </div>
            </div>

            <div className="space-y-3">
              <h3 className="text-sm font-semibold text-muted-foreground">压缩配置</h3>
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <label className="text-sm font-medium">启用压缩</label>
                  <p className="text-xs text-muted-foreground">
                    全局: {resolved?.compression?.enabled ? '开' : '关'}
                  </p>
                </div>
                <div className="flex items-center gap-2">
                  {cEnabled !== null && (
                    <Button type="button" variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => setCEnabled(null)}>
                      清除
                    </Button>
                  )}
                  <Switch
                    checked={cEnabled ?? resolved?.compression?.enabled ?? true}
                    onCheckedChange={(v) => setCEnabled(v)}
                    disabled={isPending}
                  />
                </div>
              </div>
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <label className="text-sm font-medium">空白压缩</label>
                  <p className="text-xs text-muted-foreground">
                    全局: {resolved?.compression?.whitespaceCompression ? '开' : '关'}
                  </p>
                </div>
                <div className="flex items-center gap-2">
                  {cWhitespace !== null && (
                    <Button type="button" variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => setCWhitespace(null)}>
                      清除
                    </Button>
                  )}
                  <Switch
                    checked={cWhitespace ?? resolved?.compression?.whitespaceCompression ?? true}
                    onCheckedChange={(v) => setCWhitespace(v)}
                    disabled={isPending}
                  />
                </div>
              </div>
              <div className="space-y-1">
                <label htmlFor="gcThinking" className="text-sm font-medium">Thinking 策略</label>
                <select
                  id="gcThinking"
                  className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                  value={cThinkingStrategy}
                  onChange={(e) => setCThinkingStrategy(e.target.value)}
                  disabled={isPending}
                >
                  <option value="">继承全局 ({resolved?.compression?.thinkingStrategy ?? 'discard'})</option>
                  <option value="discard">discard</option>
                  <option value="truncate">truncate</option>
                  <option value="keep">keep</option>
                </select>
              </div>
              {numInput('gcTrMaxChars', 'tool_result 截断阈值（字符）', cToolResultMaxChars, setCToolResultMaxChars, resolved?.compression?.toolResultMaxChars)}
              <div className="grid grid-cols-2 gap-2">
                {numInput('gcTrHead', '保留头部行数', cToolResultHeadLines, setCToolResultHeadLines, resolved?.compression?.toolResultHeadLines)}
                {numInput('gcTrTail', '保留尾部行数', cToolResultTailLines, setCToolResultTailLines, resolved?.compression?.toolResultTailLines)}
              </div>
              {numInput('gcTuMaxChars', 'tool_use input 截断阈值', cToolUseInputMaxChars, setCToolUseInputMaxChars, resolved?.compression?.toolUseInputMaxChars)}
              {numInput('gcTdMaxChars', '工具描述截断阈值', cToolDescMaxChars, setCToolDescMaxChars, resolved?.compression?.toolDescriptionMaxChars)}
              <div className="grid grid-cols-2 gap-2">
                {numInput('gcMaxTurns', '历史最大轮数', cMaxHistoryTurns, setCMaxHistoryTurns, resolved?.compression?.maxHistoryTurns)}
                {numInput('gcMaxChars', '历史最大字符数', cMaxHistoryChars, setCMaxHistoryChars, resolved?.compression?.maxHistoryChars)}
              </div>
              {numInput('gcMaxBody', '请求体上限（字节）', cMaxRequestBodyBytes, setCMaxRequestBodyBytes, resolved?.compression?.maxRequestBodyBytes)}
              {numInput('gcMaxInputTokens', '输入 token 上限', cMaxInputTokens, setCMaxInputTokens, resolved?.compression?.maxInputTokens)}
            </div>

            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={isPending}>
                取消
              </Button>
              <Button type="submit" disabled={isPending}>
                {isPending ? '保存中...' : '保存'}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
}
