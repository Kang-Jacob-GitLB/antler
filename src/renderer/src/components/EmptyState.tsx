import { useT } from '../i18n'
import { useAppStore } from '../stores/appStore'
import { AntlerIllustration } from './brand'
import { IconPlus } from './icons'

export function EmptyState(): React.JSX.Element {
  const t = useT()
  const addLocalRepo = useAppStore((s) => s.addLocalRepo)
  return (
    <div className="empty-state">
      <div className="empty-state__art">
        <AntlerIllustration />
      </div>
      <h1 className="empty-state__title">{t('empty.title')}</h1>
      <p className="empty-state__body">{t('empty.body')}</p>
      <button className="btn btn--primary" onClick={() => void addLocalRepo()}>
        <IconPlus size={14} />
        {t('empty.cta')}
      </button>
    </div>
  )
}
