import { contextBridge, ipcRenderer } from 'electron'
import { IPC_CHANNELS, IPC_EVENTS, type AntlerApi, type IpcEnvelope } from '@shared/ipc'

const channels = new Set<string>(IPC_CHANNELS)
const events = new Set<string>(IPC_EVENTS)

const api: AntlerApi = {
  platform: process.platform,
  async invoke(channel, ...args) {
    if (!channels.has(channel)) throw new Error(`blocked channel: ${channel}`)
    const res = (await ipcRenderer.invoke(channel, ...args)) as IpcEnvelope<never>
    if (res.ok) return res.data
    // contextBridge 는 Error 의 커스텀 필드를 떨어뜨리므로 코드는 메시지 앞에 붙인다
    throw new Error(`[${res.code ?? 'ERROR'}] ${res.error}`)
  },
  on(event, listener) {
    if (!events.has(event)) throw new Error(`blocked event: ${event}`)
    const wrapped = (_e: Electron.IpcRendererEvent, payload: Parameters<typeof listener>[0]): void => listener(payload)
    ipcRenderer.on(event, wrapped)
    return () => ipcRenderer.removeListener(event, wrapped)
  }
}

contextBridge.exposeInMainWorld('antler', api)
