import { useState, useEffect, useRef } from 'react'
import { toast } from 'sonner'
import { ExternalLink, CheckCircle, Loader2 } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { startExternalIdpLogin, pollSocialLogin } from '@/api/credentials'
import { extractErrorMessage } from '@/lib/utils'

interface ExternalIdpLoginDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSuccess: () => void
}

type Step = 'form' | 'waiting' | 'done'

const POLL_INTERVAL_MS = 2000

export function ExternalIdpLoginDialog({ open, onOpenChange, onSuccess }: ExternalIdpLoginDialogProps) {
  const [step, setStep] = useState<Step>('form')
  const [issuerUrl, setIssuerUrl] = useState('https://login.microsoftonline.com/')
  const [clientId, setClientId] = useState('')
  const [region, setRegion] = useState('us-east-1')
  const [loading, setLoading] = useState(false)
  const pollRef = useRef<ReturnType<typeof setInterval>>(undefined)

  useEffect(() => {
    if (!open) {
      setStep('form')
      setLoading(false)
      if (pollRef.current) clearInterval(pollRef.current)
    }
  }, [open])

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current)
    }
  }, [])

  const handleStart = async () => {
    if (!issuerUrl.trim() || !clientId.trim()) {
      toast.error('请填写 Issuer URL 和 Client ID')
      return
    }

    setLoading(true)
    try {
      const resp = await startExternalIdpLogin({
        issuerUrl: issuerUrl.trim(),
        clientId: clientId.trim(),
        region: region.trim() || undefined,
      })

      setStep('waiting')

      // 后端返回的 authUrl 在 portalUrl 字段（复用 StartSocialLoginResponse）
      if (resp.portalUrl) {
        window.open(resp.portalUrl, '_blank')
      }

      pollRef.current = setInterval(async () => {
        try {
          const pollResp = await pollSocialLogin(resp.sessionId)
          if (pollResp.status === 'success') {
            clearInterval(pollRef.current!)
            setStep('done')
            toast.success('External IdP 登录成功！')
            onSuccess()
            setTimeout(() => onOpenChange(false), 1500)
          } else if (pollResp.status === 'expired') {
            clearInterval(pollRef.current!)
            toast.error('登录会话已过期，请重试')
            setStep('form')
          }
        } catch {
          // polling error, continue
        }
      }, POLL_INTERVAL_MS)
    } catch (e) {
      toast.error(`External IdP 登录失败: ${extractErrorMessage(e)}`)
    } finally {
      setLoading(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>External IdP (Azure AD) 登录</DialogTitle>
          <DialogDescription>
            通过 Microsoft Entra ID / Azure AD 企业 SSO 登录
          </DialogDescription>
        </DialogHeader>

        {step === 'form' && (
          <div className="space-y-4">
            <div className="space-y-2">
              <label className="text-sm font-medium">Issuer URL</label>
              <Input
                value={issuerUrl}
                onChange={e => setIssuerUrl(e.target.value)}
                placeholder="https://login.microsoftonline.com/{tenant-id}/v2.0"
              />
              <p className="text-xs text-muted-foreground">
                Azure AD 租户的 OIDC Issuer URL
              </p>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Client ID</label>
              <Input
                value={clientId}
                onChange={e => setClientId(e.target.value)}
                placeholder="770186cb-6f63-4eb2-92e4-115e796ef02f"
              />
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Region</label>
              <Input
                value={region}
                onChange={e => setRegion(e.target.value)}
                placeholder="us-east-1"
              />
            </div>
          </div>
        )}

        {step === 'waiting' && (
          <div className="flex flex-col items-center gap-4 py-6">
            <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
            <p className="text-sm text-muted-foreground">
              请在浏览器中完成 Azure AD 登录...
            </p>
            <p className="text-xs text-muted-foreground">
              登录完成后此对话框会自动关闭
            </p>
          </div>
        )}

        {step === 'done' && (
          <div className="flex flex-col items-center gap-4 py-6">
            <CheckCircle className="h-8 w-8 text-green-500" />
            <p className="text-sm font-medium">External IdP 登录成功！</p>
          </div>
        )}

        <DialogFooter>
          {step === 'form' && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                取消
              </Button>
              <Button onClick={handleStart} disabled={loading}>
                {loading ? (
                  <Loader2 className="h-4 w-4 animate-spin mr-2" />
                ) : (
                  <ExternalLink className="h-4 w-4 mr-2" />
                )}
                开始登录
              </Button>
            </>
          )}
          {step === 'waiting' && (
            <Button variant="outline" onClick={() => {
              if (pollRef.current) clearInterval(pollRef.current)
              setStep('form')
            }}>
              取消
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
