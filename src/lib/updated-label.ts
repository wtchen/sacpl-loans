// Pure helpers behind the footer's "Updated …" text and the stale-on-open
// refresh decision. Timestamps are ms since the epoch; `now` is always a
// parameter so callers (and tests) can pin the clock.

export const DAY_MS = 24 * 60 * 60 * 1000;

/** Local-midnight timestamp of the given instant. */
export function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/**
 * "Sep 6 2026" — month-first date form used by the >6-day label and the
 * hover tooltip. The year is shown only when the date falls in a previous
 * calendar year than `now`. Locale is injectable for tests.
 */
export function stampDate(ms: number, now: number, locale?: string): string {
  const d = new Date(ms);
  const month = d.toLocaleString(locale, { month: "short" });
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return sameYear ? `${month} ${d.getDate()}` : `${month} ${d.getDate()} ${d.getFullYear()}`;
}

/** "Sep 6 10:15 PM" (or 24h, per the user's locale) — hover tooltip. */
export function fullStamp(ms: number, now: number, locale?: string): string {
  const d = new Date(ms);
  const time = d.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" });
  return `${stampDate(ms, now, locale)} ${time}`;
}

/**
 * Footer label for when the list was last refreshed:
 * same day → "Updated <time>"; 1 day → "Updated yesterday";
 * 2–6 days → "Updated N days ago"; otherwise "Updated <date>".
 * "" when never refreshed. `formatTime` is injectable for tests.
 */
export function updatedLabel(
  lastUpdated: number,
  now: number,
  formatTime: (ms: number) => string = (ms) =>
    new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit", second: "2-digit" }),
  locale?: string
): string {
  if (!lastUpdated) return "";
  const days = Math.floor((startOfDay(now) - startOfDay(lastUpdated)) / DAY_MS);
  if (days <= 0) return `Updated ${formatTime(lastUpdated)}`;
  if (days === 1) return "Updated yesterday";
  if (days <= 6) return `Updated ${days} days ago`;
  return `Updated ${stampDate(lastUpdated, now, locale)}`;
}

/** True when the list is missing or older than the refresh interval. */
export function isStale(lastUpdated: number, now: number, intervalMs: number): boolean {
  if (!lastUpdated) return true;
  return now - lastUpdated > intervalMs;
}