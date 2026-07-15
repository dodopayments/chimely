import type { ReactNode } from 'react';
import { useEffect, useMemo } from 'react';
import type { InboxAppearance } from '../appearance';
import { slotClass, variablesToStyle } from '../appearance';
import { useNotifications, usePreferences } from '../hooks';
import type { InboxLocalization } from '../localization';
import { mergeLocalization } from '../localization';
import { ensureStyles } from '../styles';

/** A labeled group of preference category rows. */
export interface PreferenceGroup {
  /** Heading shown above the group's rows. */
  label: string;
  /** Category keys in this group, in display order. */
  categories: ReadonlyArray<string>;
}

/**
 * Standalone per-category in_app preference toggles, for settings pages and
 * custom popovers. Reads the client from the enclosing ChimelyProvider.
 * Categories are derived from loaded items plus explicit preference rows.
 */
export interface PreferencesProps {
  appearance?: InboxAppearance;
  localization?: Partial<InboxLocalization>;
  /** Show only categories for which this returns true. Default: all shown. */
  preferencesFilter?: (category: string) => boolean;
  /** Order the category rows. Default: alphabetical by category key. */
  preferencesSort?: (a: string, b: string) => number;
  /**
   * Group category rows under labeled headings, in the given order. Categories
   * absent from every group render after the groups in the sorted order.
   */
  preferenceGroups?: ReadonlyArray<PreferenceGroup>;
}

export function Preferences(props: PreferencesProps): ReactNode {
  const { items } = useNotifications();
  const preferences = usePreferences();
  const strings = mergeLocalization(props.localization);
  const { preferencesFilter, preferencesSort, preferenceGroups } = props;

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

  // Filter then sort. Default order is the alphabetical `categories` above.
  const visible = useMemo(() => {
    const filtered = preferencesFilter ? categories.filter(preferencesFilter) : categories;
    return preferencesSort ? [...filtered].sort(preferencesSort) : filtered;
  }, [categories, preferencesFilter, preferencesSort]);

  const isEnabled = (category: string): boolean =>
    !preferences.preferences.some(
      (row) => row.category === category && row.channel === 'in_app' && !row.enabled,
    );

  const renderRow = (category: string): ReactNode => (
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
  );

  const visibleSet = new Set(visible);
  const groups = preferenceGroups ?? [];
  const grouped = new Set(groups.flatMap((group) => group.categories));
  const ungrouped = visible.filter((category) => !grouped.has(category));

  return (
    <div
      className={slotClass(props.appearance?.classNames, 'preferences')}
      style={variablesToStyle(props.appearance?.variables)}
    >
      {groups.map((group) => {
        const rows = group.categories.filter((category) => visibleSet.has(category));
        if (rows.length === 0) {
          return null;
        }
        return (
          <div key={group.label} className="chimely-preference-group">
            <div className="chimely-preference-group-label">{group.label}</div>
            {rows.map(renderRow)}
          </div>
        );
      })}
      {ungrouped.map(renderRow)}
    </div>
  );
}
