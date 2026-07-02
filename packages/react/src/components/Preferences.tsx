import type { ReactNode } from 'react';
import { useEffect, useMemo } from 'react';
import type { InboxAppearance } from '../appearance';
import { slotClass, variablesToStyle } from '../appearance';
import { useNotifications, usePreferences } from '../hooks';
import type { InboxLocalization } from '../localization';
import { mergeLocalization } from '../localization';
import { ensureStyles } from '../styles';

/**
 * Standalone per-category in_app preference toggles, for settings pages and
 * custom popovers. Reads the client from the enclosing ChimelyProvider.
 * Categories are derived from loaded items plus explicit preference rows.
 */
export interface PreferencesProps {
  appearance?: InboxAppearance;
  localization?: Partial<InboxLocalization>;
}

export function Preferences(props: PreferencesProps): ReactNode {
  const { items } = useNotifications();
  const preferences = usePreferences();
  const strings = mergeLocalization(props.localization);

  useEffect(() => {
    ensureStyles();
  }, []);

  const categories = useMemo(() => {
    const set = new Set<string>();
    for (const item of items) {
      set.add(item.category);
    }
    for (const row of preferences.preferences) {
      set.add(row.category);
    }
    return [...set].sort();
  }, [items, preferences.preferences]);

  const isEnabled = (category: string): boolean =>
    !preferences.preferences.some(
      (row) => row.category === category && row.channel === 'in_app' && !row.enabled,
    );

  return (
    <div
      className={slotClass(props.appearance?.classNames, 'preferences')}
      style={variablesToStyle(props.appearance?.variables)}
    >
      {categories.map((category) => (
        <label key={category} className="chimely-preference">
          <span>{strings.categoryLabels[category] ?? category}</span>
          <input
            type="checkbox"
            checked={isEnabled(category)}
            onChange={(event) => {
              void preferences.setPreferences([
                { category, channel: 'in_app', enabled: event.target.checked },
              ]);
            }}
          />
        </label>
      ))}
    </div>
  );
}
