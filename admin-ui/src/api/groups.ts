import axios from 'axios'
import { storage } from '@/lib/storage'
import type {
  GroupsResponse,
  GroupItem,
  CreateGroupRequest,
  UpdateGroupRequest,
  SuccessResponse,
} from '@/types/api'

const api = axios.create({
  baseURL: '/api/admin',
  timeout: 15000,
  headers: { 'Content-Type': 'application/json' },
})

api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey()
  if (apiKey) config.headers['x-api-key'] = apiKey
  return config
})

export async function listGroups(): Promise<GroupsResponse> {
  const { data } = await api.get<GroupsResponse>('/groups')
  return data
}

export async function createGroup(req: CreateGroupRequest): Promise<GroupItem> {
  const { data } = await api.post<GroupItem>('/groups', req)
  return data
}

export async function updateGroup(
  name: string,
  req: UpdateGroupRequest,
): Promise<GroupItem> {
  // path 中的分组名可能含中文/空格，必须 encodeURIComponent
  const { data } = await api.patch<GroupItem>(`/groups/${encodeURIComponent(name)}`, req)
  return data
}

/** 删除分组。`force=true` 时级联清理所有引用（凭据 groups + 客户端 Key.group）。 */
export async function deleteGroup(name: string, force = false): Promise<SuccessResponse> {
  const path = `/groups/${encodeURIComponent(name)}${force ? '?force=true' : ''}`
  const { data } = await api.delete<SuccessResponse>(path)
  return data
}

export interface GroupConfigResponse {
  groupName: string
  overrides: GroupConfigOverrides
  resolved: ResolvedGroupConfig
}

export interface GroupConfigOverrides {
  credentialRpm?: number | null
  credentialDailyMaxRequests?: number | null
  defaultEndpoint?: string | null
  promptCacheTtlSeconds?: number | null
  compression?: GroupCompressionOverrides | null
}

export interface GroupCompressionOverrides {
  enabled?: boolean | null
  whitespaceCompression?: boolean | null
  thinkingStrategy?: string | null
  toolResultMaxChars?: number | null
  toolResultHeadLines?: number | null
  toolResultTailLines?: number | null
  toolUseInputMaxChars?: number | null
  toolDescriptionMaxChars?: number | null
  maxHistoryTurns?: number | null
  maxHistoryChars?: number | null
  maxRequestBodyBytes?: number | null
}

export interface ResolvedGroupConfig {
  credentialRpm: number | null
  credentialDailyMaxRequests: number | null
  defaultEndpoint: string
  promptCacheTtlSeconds: number
  compression: {
    enabled: boolean
    whitespaceCompression: boolean
    thinkingStrategy: string
    toolResultMaxChars: number
    toolResultHeadLines: number
    toolResultTailLines: number
    toolUseInputMaxChars: number
    toolDescriptionMaxChars: number
    maxHistoryTurns: number
    maxHistoryChars: number
    maxRequestBodyBytes: number
  }
}

export async function fetchGroupConfig(name: string): Promise<GroupConfigResponse> {
  const { data } = await api.get<GroupConfigResponse>(`/groups/${encodeURIComponent(name)}/config`)
  return data
}

export async function updateGroupConfig(
  name: string,
  config: GroupConfigOverrides,
): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>(
    `/groups/${encodeURIComponent(name)}/config`,
    config,
  )
  return data
}
