import type { Locale } from '@shared/types'
import { useAppStore } from '../stores/appStore'
import { en } from './en'
import { ko, type Messages } from './ko'

export type MessageKey = keyof Messages

const dicts: Record<Locale, Messages> = { ko, en }

export function translate(locale: Locale, key: MessageKey, vars?: Record<string, string | number>): string {
  const text = dicts[locale][key] ?? ko[key]
  return vars ? text.replace(/\{(\w+)\}/g, (_, k: string) => String(vars[k] ?? `{${k}}`)) : text
}

export function useT(): (key: MessageKey, vars?: Record<string, string | number>) => string {
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  return (key, vars) => translate(locale, key, vars)
}
