import type { InboxCountFilter, InboxItemId } from '@chimely/client';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useChimelyClient } from './context';

interface CategoryTab {
  categories?: ReadonlyArray<string>;
}

interface CountConfiguration {
  filters: InboxCountFilter[];
  tabIndexes: number[];
}

interface ResolvedCounts {
  configuration: CountConfiguration;
  values: ReadonlyMap<number, number>;
}

const EMPTY_COUNTS: ReadonlyMap<number, number> = new Map();

function countConfiguration(tabs: ReadonlyArray<CategoryTab> | undefined): CountConfiguration {
  const filters: InboxCountFilter[] = [];
  const tabIndexes: number[] = [];
  for (const [index, tab] of (tabs ?? []).entries()) {
    if (tab.categories !== undefined) {
      filters.push({ categories: tab.categories });
      tabIndexes.push(index);
    }
  }
  return { filters, tabIndexes };
}

/** Exact counts for category tabs, refreshed after each server list merge. */
export function useExactTabCounts(
  tabs: ReadonlyArray<CategoryTab> | undefined,
  refreshSignal: ReadonlyArray<InboxItemId> | undefined,
): { counts: ReadonlyMap<number, number>; refresh: () => Promise<void> } {
  const client = useChimelyClient();
  const configurationKey = JSON.stringify((tabs ?? []).map((tab) => tab.categories ?? null));
  // biome-ignore lint/correctness/useExhaustiveDependencies: the serialized category configuration is the dependency
  const configuration = useMemo(() => countConfiguration(tabs), [configurationKey]);
  const [resolved, setResolved] = useState<ResolvedCounts>();
  const requestGeneration = useRef(0);

  const refresh = useCallback(async () => {
    const generation = ++requestGeneration.current;
    if (configuration.filters.length === 0) {
      return;
    }
    try {
      const response = await client.getFilteredCounts(configuration.filters);
      if (
        generation === requestGeneration.current &&
        response.length === configuration.tabIndexes.length
      ) {
        setResolved({
          configuration,
          values: new Map(
            configuration.tabIndexes.map((tabIndex, position) => [
              tabIndex,
              response[position]?.unread ?? 0,
            ]),
          ),
        });
      }
    } catch {
      // The last confirmed counts remain usable during a transient failure.
    }
  }, [client, configuration]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshSignal changes only after a server list merge
  useEffect(() => {
    void refresh();
    return () => {
      requestGeneration.current += 1;
    };
  }, [refresh, refreshSignal]);

  return {
    counts: resolved?.configuration === configuration ? resolved.values : EMPTY_COUNTS,
    refresh,
  };
}
