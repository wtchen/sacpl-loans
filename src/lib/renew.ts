// Pure renewal logic behind the Renew button (unit-tested; the panel wires
// it into its state and commands).

export type RenewState = {
  busy: boolean; // a refresh/login request is in flight
  reconnecting: boolean; // the backend is re-authenticating an expired session
  hasCache: boolean; // the list on screen came from the on-disk cache
  lastFetchAt: number; // ms epoch of the last FRESH fetch (0 = never)
};

/** Blocked while a request is in flight or the list is unverified cache. */
export function isRenewBlocked(state: RenewState): boolean {
  return state.busy || state.reconnecting || (state.hasCache && state.lastFetchAt === 0);
}

/** The fake renewal for debug books: due date +3 weeks, catalog's format. */
export function simulatedRenewalDue(now: number, locale?: string): string {
  const due = new Date(now);
  due.setDate(due.getDate() + 21);
  return `Due ${due.toLocaleDateString(locale, { month: "short", day: "numeric", year: "numeric" })}`;
}