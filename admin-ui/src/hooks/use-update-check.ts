import { useQuery } from '@tanstack/react-query'
import { checkSystemUpdate } from '@/api/credentials'

/**
 * 轮询后端"检查更新"接口。
 *
 * 后端命中缓存时直接返回，未命中才会调用上游版本接口。前端这里再叠加一层
 * 15 分钟的 refetchInterval，足以让用户在打开页面后短时间内看到红点提醒，
 * 又不会带来明显的请求压力。
 */
export function useUpdateCheck() {
  return useQuery({
    queryKey: ['system-update-check'],
    queryFn: () => checkSystemUpdate(false),
    // 15 分钟主动刷新一次；首次加载时立即执行
    refetchInterval: 15 * 60 * 1000,
    // 避免短时间内反复请求
    staleTime: 5 * 60 * 1000,
    // 网络抖动时不要疯狂重试
    retry: 1,
  })
}
