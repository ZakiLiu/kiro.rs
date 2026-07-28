import { useState } from 'react'
import { toast } from 'sonner'
import { Boxes, PlayCircle, RefreshCw } from 'lucide-react'
import { Card, CardContent } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { usePoolModels, useTestPoolModel } from '@/hooks/use-credentials'
import { extractErrorMessage, formatNumber } from '@/lib/utils'
import type { AdminTestModelResponse } from '@/types/api'

/**
 * 模型面板：按账号池策略查询上游模型目录 + 真实请求测试
 *
 * 与"凭据卡片 · 可用模型"对话框的分工：
 * - 该面板：看账号池整体能调用哪些模型（走上游动态目录合并）
 * - 凭据卡片：看单个凭据支持哪些模型（用于排查具体账号能力差异）
 */
export function ModelsPage() {
  const { data, isLoading, isFetching, refetch, error } = usePoolModels(true)
  const testMutation = useTestPoolModel()

  const [testOpen, setTestOpen] = useState(false)
  const [testModel, setTestModel] = useState<string>('')
  const [testResult, setTestResult] = useState<AdminTestModelResponse | null>(
    null,
  )
  const [testError, setTestError] = useState<string | null>(null)

  const openTest = (model: string) => {
    setTestModel(model)
    setTestResult(null)
    setTestError(null)
    setTestOpen(true)
  }

  const runTest = async () => {
    if (!testModel) return
    setTestResult(null)
    setTestError(null)
    try {
      const resp = await testMutation.mutateAsync({ model: testModel })
      setTestResult(resp)
      toast.success(`模型 ${testModel} 测试成功`)
    } catch (e) {
      const msg = extractErrorMessage(e)
      setTestError(msg)
      toast.error(`测试失败: ${msg}`)
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Boxes className="h-5 w-5 text-primary" />
          <h2 className="text-lg font-semibold">账号池模型</h2>
          {data && (
            <Badge variant="secondary" className="tabular-nums">
              {data.models.length} 个
            </Badge>
          )}
          {data && (
            <Badge variant="outline">
              {data.selection === 'balanced' ? '均衡模式' : '优先级模式'}
            </Badge>
          )}
          {data && (
            <span className="text-xs text-muted-foreground">
              选中凭据 #{data.credentialId}
            </span>
          )}
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={() => refetch()}
          disabled={isFetching}
        >
          <RefreshCw
            className={`h-3.5 w-3.5 ${isFetching ? 'animate-spin' : ''}`}
          />
          <span className="ml-1.5">刷新</span>
        </Button>
      </div>

      <p className="text-xs text-muted-foreground">
        按当前账号池策略选一个未禁用凭据，展示其上游 ListAvailableModels 目录（走
        TTL 缓存，不改写调度指针）。点击右侧"测试"按钮向该模型发一个最小化请求验证实际可调用性。
      </p>

      {isLoading && (
        <div className="flex items-center justify-center py-12">
          <div className="h-8 w-8 animate-spin rounded-full border-b-2 border-primary" />
        </div>
      )}

      {error && !isLoading && (
        <Card>
          <CardContent className="py-6 text-center text-sm text-red-500">
            {extractErrorMessage(error)}
          </CardContent>
        </Card>
      )}

      {data && data.models.length === 0 && (
        <Card>
          <CardContent className="py-8 text-center text-sm text-muted-foreground">
            账号池当前没有可用模型
          </CardContent>
        </Card>
      )}

      {data && data.models.length > 0 && (
        <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
          {data.models.map((m) => (
            <Card key={m.modelId}>
              <CardContent className="space-y-1.5 py-3">
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <div className="font-medium text-sm truncate">
                      {m.modelName || m.modelId}
                    </div>
                    {m.modelName && m.modelName !== m.modelId && (
                      <div className="mt-0.5 font-mono text-[11px] text-muted-foreground truncate">
                        {m.modelId}
                      </div>
                    )}
                  </div>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-7 shrink-0 px-2"
                    onClick={() => openTest(m.modelId)}
                    title="真实请求测试"
                  >
                    <PlayCircle className="h-3.5 w-3.5" />
                    <span className="ml-1 text-xs">测试</span>
                  </Button>
                </div>
                <div className="flex flex-wrap items-center gap-1.5">
                  {m.maxInputTokens != null && (
                    <Badge variant="secondary" className="tabular-nums">
                      输入 {formatNumber(m.maxInputTokens)}
                    </Badge>
                  )}
                  {m.maxOutputTokens != null && (
                    <Badge variant="secondary" className="tabular-nums">
                      输出 {formatNumber(m.maxOutputTokens)}
                    </Badge>
                  )}
                </div>
                {m.description && (
                  <div className="text-xs text-muted-foreground">
                    {m.description}
                  </div>
                )}
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      <Dialog open={testOpen} onOpenChange={setTestOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>测试模型 · {testModel}</DialogTitle>
          </DialogHeader>

          {!testResult && !testError && (
            <div className="space-y-2 py-2 text-sm">
              <p className="text-muted-foreground">
                将用账号池策略选中的凭据发一个最小化请求（max_tokens=8，content=&quot;ping&quot;）
                到该模型，展示响应文本与端到端耗时。
              </p>
            </div>
          )}

          {testMutation.isPending && (
            <div className="flex items-center justify-center py-6">
              <div className="h-6 w-6 animate-spin rounded-full border-b-2 border-primary" />
              <span className="ml-2 text-xs text-muted-foreground">
                请求中…
              </span>
            </div>
          )}

          {testResult && (
            <div className="space-y-3 py-2 text-sm">
              <div className="flex items-center gap-2">
                <Badge variant="secondary">凭据 #{testResult.credentialId}</Badge>
                <Badge variant="outline" className="tabular-nums">
                  {testResult.latencyMs} ms
                </Badge>
                {testResult.creditUsage != null && (
                  <Badge variant="outline" className="tabular-nums">
                    {testResult.creditUsage} {testResult.creditUnit ?? ''}
                  </Badge>
                )}
              </div>
              <div className="max-h-64 overflow-auto rounded border border-border/60 bg-secondary/30 px-3 py-2 font-mono text-[11px] whitespace-pre-wrap break-all">
                {testResult.text || '(响应无可见文本)'}
              </div>
            </div>
          )}

          {testError && (
            <div className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-500">
              {testError}
            </div>
          )}

          <DialogFooter>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setTestOpen(false)}
            >
              关闭
            </Button>
            <Button
              size="sm"
              onClick={runTest}
              disabled={testMutation.isPending}
            >
              {testMutation.isPending ? '测试中…' : '开始测试'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
