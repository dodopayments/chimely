import { useEffect, useState } from 'react';

let relativeFormatter: Intl.RelativeTimeFormat | undefined;

function formatter(): Intl.RelativeTimeFormat {
  relativeFormatter ??= new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
  return relativeFormatter;
}

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;
const WEEK = 7 * DAY;

/**
 * Relative form of an RFC 3339 timestamp: "now" under a minute, then
 * minutes/hours/days via Intl.RelativeTimeFormat, then the locale date past
 * seven days. Invalid input is returned verbatim.
 */
export function formatRelativeTime(iso: string, nowMs?: number): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) {
    return iso;
  }
  const deltaSeconds = Math.round((then - (nowMs ?? Date.now())) / 1000);
  const magnitude = Math.abs(deltaSeconds);
  if (magnitude < MINUTE) {
    return 'now';
  }
  if (magnitude < HOUR) {
    return formatter().format(Math.trunc(deltaSeconds / MINUTE), 'minute');
  }
  if (magnitude < DAY) {
    return formatter().format(Math.trunc(deltaSeconds / HOUR), 'hour');
  }
  if (magnitude < WEEK) {
    return formatter().format(Math.trunc(deltaSeconds / DAY), 'day');
  }
  return new Date(then).toLocaleDateString();
}

/**
 * Re-renders the caller on an interval so relative timestamps stay current.
 * Pass null to pause. Returns the current epoch milliseconds.
 */
export function useNow(intervalMs: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (intervalMs === null) {
      return undefined;
    }
    setNow(Date.now());
    const timer = setInterval(() => {
      setNow(Date.now());
    }, intervalMs);
    return () => {
      clearInterval(timer);
    };
  }, [intervalMs]);
  return now;
}
