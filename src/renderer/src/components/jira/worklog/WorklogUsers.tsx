import { useEffect, useRef, useState } from 'react'
import type { JiraUser } from '@shared/types'
import { useT } from '../../../i18n'
import { api, errorMessage, isAbort, newRequestId } from '../../../lib/api'
import { MAX_TEAM, useJiraStore } from '../../../stores/jiraStore'
import { useWorklogStore } from '../../../stores/jiraWorklogStore'
import { IconClose, IconPlus, IconUser } from '../../icons'
import { userColor } from './WorklogBits'

const SEARCH_DEBOUNCE_MS = 300

/** 사용자 선택: '나' + 팀원 칩(눌러 켜고 끄기, x 로 삭제) + 팀원 관리 팝오버 */
export function WorklogUsers({ team, me }: { team: JiraUser[]; me: JiraUser | null }): React.JSX.Element {
  const t = useT()
  const includeMe = useWorklogStore((s) => s.includeMe)
  const selected = useWorklogStore((s) => s.selectedTeam)
  const { toggleMe, toggleTeam } = useWorklogStore.getState()
  const { removeTeamMember } = useJiraStore.getState()

  return (
    <div className="wl-users" role="group" aria-label={t('jira.worklog.users')}>
      <button className={`wl-user${includeMe ? ' is-on' : ''}`} aria-pressed={includeMe} title={me?.name} onClick={toggleMe}>
        <span className="wl-user__dot" style={{ background: userColor(me?.id ?? 'me') }} />
        {t('jira.worklog.me')}
      </button>
      {team.map((u) => {
        const on = selected.includes(u.id)
        return (
          <span key={u.id} className={`wl-user wl-user--team${on ? ' is-on' : ''}`}>
            <button className="wl-user__main" aria-pressed={on} title={u.name} onClick={() => toggleTeam(u.id)}>
              <span className="wl-user__dot" style={{ background: userColor(u.id) }} />
              <span className="wl-user__name">{u.name}</span>
            </button>
            <button className="wl-user__x" title={t('jira.worklog.removeMember', { name: u.name })} aria-label={t('jira.worklog.removeMember', { name: u.name })} onClick={() => removeTeamMember(u.id)}>
              <IconClose size={10} />
            </button>
          </span>
        )
      })}
      <TeamManager team={team} me={me} />
    </div>
  )
}

function TeamManager({ team, me }: { team: JiraUser[]; me: JiraUser | null }): React.JSX.Element {
  const t = useT()
  const [open, setOpen] = useState(false)
  const [text, setText] = useState('')
  const [results, setResults] = useState<JiraUser[]>([])
  const [searching, setSearching] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const box = useRef<HTMLDivElement>(null)
  const seq = useRef(0)
  const req = useRef<string | null>(null)
  const { addTeamMember } = useJiraStore.getState()

  const cancel = (): void => {
    if (req.current) void api.invoke('req:cancel', req.current).catch(() => {})
    req.current = null
  }

  // 입력이 300ms 멈추면 검색한다. 늦게 온 응답은 순번으로 버린다
  useEffect(() => {
    const q = text.trim()
    seq.current++
    cancel()
    if (!open || !q) {
      setResults([])
      setSearching(false)
      setError(null)
      return
    }
    const mine = seq.current
    setSearching(true)
    const timer = setTimeout(() => {
      const id = newRequestId('jira')
      req.current = id
      api
        .invoke('jira:userSearch', q, id)
        .then((users) => {
          if (mine !== seq.current) return
          setResults(users)
          setError(null)
          setSearching(false)
        })
        .catch((err) => {
          if (mine !== seq.current || isAbort(err)) return
          setResults([])
          setError(errorMessage(err))
          setSearching(false)
        })
    }, SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(timer)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, open])

  // 바깥을 누르면 닫는다
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  const full = team.length >= MAX_TEAM
  const shown = results.filter((u) => u.id !== me?.id && !team.some((x) => x.id === u.id))

  return (
    <div className="wl-team" ref={box}>
      <button className="btn btn--sm" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <IconUser size={13} />
        {t('jira.worklog.manage')}
      </button>
      {open && (
        <div className="wl-pop" role="dialog" aria-label={t('jira.worklog.manage')} onKeyDown={(e) => {
            if (e.key !== 'Escape') return
            // 전역 Esc(Jira 화면 닫기)까지 가지 않게 네이티브 이벤트도 멈춘다
            e.nativeEvent.stopPropagation()
            setOpen(false)
          }}>
          <input className="field__input" autoFocus value={text} spellCheck={false} placeholder={t('jira.worklog.search')} aria-label={t('jira.worklog.search')} onChange={(e) => setText(e.target.value)} />
          {full && <div className="wl-pop__note">{t('jira.worklog.maxTeam', { n: MAX_TEAM })}</div>}
          {error && (
            <div className="wl-pop__err" role="alert">
              <div>{error}</div>
              <div className="wl-pop__hint">{t('jira.worklog.searchHint')}</div>
            </div>
          )}
          {!error && text.trim() && (
            <div className="wl-pop__sec">
              <div className="wl-pop__title">{t('jira.worklog.resultsTitle')}</div>
              {searching && <div className="wl-pop__note">{t('jira.worklog.searching')}</div>}
              {!searching && shown.length === 0 && <div className="wl-pop__note">{t('jira.worklog.noResults')}</div>}
              {shown.map((u) => (
                <button key={u.id} className="wl-pop__item" disabled={full} title={t('jira.worklog.addMember', { name: u.name })} onClick={() => addTeamMember(u)}>
                  <span className="wl-user__dot" style={{ background: userColor(u.id) }} />
                  <span className="wl-pop__name">{u.name}</span>
                  <IconPlus size={12} />
                </button>
              ))}
            </div>
          )}
          <div className="wl-pop__sec">
            <div className="wl-pop__title">{t('jira.worklog.teamTitle')}</div>
            {team.length === 0 && <div className="wl-pop__note">{t('jira.worklog.teamEmpty')}</div>}
            {team.map((u) => (
              <div key={u.id} className="wl-pop__item wl-pop__item--static">
                <span className="wl-user__dot" style={{ background: userColor(u.id) }} />
                <span className="wl-pop__name">{u.name}</span>
                <button
                  className="icon-btn icon-btn--sm"
                  title={t('jira.worklog.removeMember', { name: u.name })}
                  aria-label={t('jira.worklog.removeMember', { name: u.name })}
                  onClick={() => useJiraStore.getState().removeTeamMember(u.id)}
                >
                  <IconClose size={11} />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
