import { useState, useRef } from 'react'
import { toast } from 'sonner'
import { Upload, Loader2, FileText, FolderOpen } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useAddCredential } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'

interface ExternalIdpImportDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

const EXTERNAL_IDP_ALIASES = ['external_idp', 'azuread', 'azure', 'entra', 'entra-id', 'microsoft', 'm365', 'office365', 'external']

export function ExternalIdpImportDialog({ open, onOpenChange }: ExternalIdpImportDialogProps) {
  const [jsonText, setJsonText] = useState('')
  const [parsed, setParsed] = useState<Record<string, unknown> | null>(null)
  const [importing, setImporting] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const { mutateAsync: addCredential } = useAddCredential()

  const handleParse = () => {
    if (!jsonText.trim()) {
      toast.error('请粘贴凭据 JSON')
      return
    }
    try {
      const obj = JSON.parse(jsonText.trim())
      setParsed(obj)
    } catch {
      toast.error('JSON 格式错误')
    }
  }

  const handleImport = async () => {
    if (!parsed) return
    setImporting(true)
    try {
      const cred = parsed as Record<string, string | number | boolean | undefined>

      // 兼容 camelCase + snake_case 双格式
      const refreshToken = (cred.refreshToken || cred.refresh_token || '') as string
      const accessToken = (cred.accessToken || cred.access_token || '') as string
      const clientId = (cred.clientId || cred.client_id || '') as string
      const tokenEndpoint = (cred.tokenEndpoint || cred.token_endpoint || '') as string
      const issuerUrl = (cred.issuerUrl || cred.issuer_url || '') as string
      const scopes = (cred.scopes || '') as string
      const provider = (cred.provider || '') as string
      const profileArn = (cred.profileArn || cred.profile_arn || '') as string
      const region = (cred.region || 'us-east-1') as string
      const rawAuthMethod = ((cred.authMethod || cred.auth_method || '') as string).toLowerCase()

      if (!refreshToken) {
        toast.error('缺少 refreshToken / refresh_token')
        return
      }

      // 检测 auth_method
      let authMethod: 'external_idp' | 'social' | 'idc' | 'api_key' = 'external_idp'
      if (EXTERNAL_IDP_ALIASES.includes(rawAuthMethod) || tokenEndpoint) {
        authMethod = 'external_idp'
      } else if (rawAuthMethod) {
        toast.warning(`检测到 authMethod="${rawAuthMethod}"，非 external_idp，仍按 external_idp 导入`)
      }

      const result = await addCredential({
        refreshToken: refreshToken.trim(),
        accessToken: accessToken.trim() || undefined,
        authMethod,
        clientId: clientId.trim() || undefined,
        region: region.trim(),
        tokenEndpoint: tokenEndpoint.trim() || undefined,
        issuerUrl: issuerUrl.trim() || undefined,
        scopes: scopes.trim() || undefined,
        provider: provider.trim() || 'ExternalIdp',
        profileArn: profileArn.trim() || undefined,
      })

      toast.success(`External IdP 凭据导入成功！凭据 ID: ${result.credentialId}`)
      onOpenChange(false)
      setJsonText('')
      setParsed(null)
    } catch (e) {
      toast.error(`导入失败: ${extractErrorMessage(e)}`)
    } finally {
      setImporting(false)
    }
  }

  const getDisplayValue = (key: string) => {
    if (!parsed) return ''
    const val = (parsed[key] ?? '') as string
    if (!val) return ''
    if (val.length > 40) return val.slice(0, 20) + '...' + val.slice(-10)
    return val
  }

  return (
    <Dialog open={open} onOpenChange={(v) => {
      if (!v) { setParsed(null); setJsonText('') }
      onOpenChange(v)
    }}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>导入 External IdP 凭据</DialogTitle>
          <DialogDescription>
            粘贴 Kiro IDE 或 kiro-login-helper 导出的 External IdP (Azure AD) 凭据 JSON
          </DialogDescription>
        </DialogHeader>

        {!parsed ? (
          <div className="space-y-3">
            <Textarea
              value={jsonText}
              onChange={e => setJsonText(e.target.value)}
              placeholder={'粘贴 External IdP 凭据 JSON，或点击下方按钮选择文件\n\n支持两种格式：\n• Kiro IDE 格式（camelCase）\n• kiro-login-helper 格式（snake_case）'}
              className="min-h-[180px] font-mono text-xs"
            />
            <input
              ref={fileInputRef}
              type="file"
              accept=".json"
              className="hidden"
              onChange={e => {
                const file = e.target.files?.[0]
                if (!file) return
                const reader = new FileReader()
                reader.onload = ev => {
                  const text = ev.target?.result as string
                  if (text) setJsonText(text)
                }
                reader.readAsText(file)
                e.target.value = ''
              }}
            />
            <Button variant="outline" size="sm" className="w-full" onClick={() => fileInputRef.current?.click()}>
              <FolderOpen className="h-3.5 w-3.5 mr-2" />
              选择 JSON 文件
            </Button>
          </div>
        ) : (
          <div className="space-y-2 text-sm">
            <div className="rounded-md border p-3 space-y-1.5 bg-muted/30">
              <div className="flex justify-between">
                <span className="text-muted-foreground">Auth Method</span>
                <span className="font-medium">{getDisplayValue('authMethod') || getDisplayValue('auth_method') || 'external_idp'}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Client ID</span>
                <span className="font-mono text-xs">{getDisplayValue('clientId') || getDisplayValue('client_id') || '—'}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Token Endpoint</span>
                <span className="font-mono text-xs truncate max-w-[250px]">{getDisplayValue('tokenEndpoint') || getDisplayValue('token_endpoint') || '—'}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Provider</span>
                <span>{getDisplayValue('provider') || 'ExternalIdp'}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Region</span>
                <span>{getDisplayValue('region') || 'us-east-1'}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Refresh Token</span>
                <span className="font-mono text-xs">{(getDisplayValue('refreshToken') || getDisplayValue('refresh_token')) ? '✅ 有' : '❌ 缺'}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Access Token</span>
                <span className="font-mono text-xs">{(getDisplayValue('accessToken') || getDisplayValue('access_token')) ? '✅ 有 (trust-on-import)' : '—'}</span>
              </div>
            </div>
          </div>
        )}

        <DialogFooter>
          {!parsed ? (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>取消</Button>
              <Button onClick={handleParse} disabled={!jsonText.trim()}>
                <FileText className="h-4 w-4 mr-2" />
                解析
              </Button>
            </>
          ) : (
            <>
              <Button variant="outline" onClick={() => setParsed(null)}>返回修改</Button>
              <Button onClick={handleImport} disabled={importing}>
                {importing ? <Loader2 className="h-4 w-4 animate-spin mr-2" /> : <Upload className="h-4 w-4 mr-2" />}
                导入凭据
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
