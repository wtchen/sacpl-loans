// Pure renewal logic behind the Renew button, kept free of UI/Tauri so it
// can be unit tested. The panel wires these into its state and commands.

/** Everything a renewal needs to know about the app's current state. */
export type RenewState = {
  /** A refresh/login/etc. request is in flight. */
  busy: boolean;
  /** The backend is re-authenticating an expired session. */
  reconnecting: boolean;
  /** The list on screen came from the on-disk cache. */
  hasCache: boolean;
  /** When a FRESH checkout list was last fetched (ms epoch; 0 = never). */
  lastFetchAt: number;
};

/**
 * True when renewals must not be attempted: a refresh is in flight, the
 * backend is re-authenticating, or the on-screen list is only the disk cache
 * (we don't yet know we're logged in, so the session is unverified).
 */
export function isRenewBlocked(state: RenewState): boolean {
  return state.busy || state.reconnecting || (state.hasCache && state.lastFetchAt === 0);
}

/**
 * The fake renewal used for debug books: due date moved three weeks ahead,
 * formatted like the catalog's due labels ("Due Sep 27, 2026"). Never touches
 * the real catalog. Locale is injectable so tests can pin a value.
 */
export function simulatedRenewalDue(now: number, locale?: string): string {
  const due = new Date(now);
  due.setDate(due.getDate() + 21);
  return `Due ${due.toLocaleDateString(locale, { month: "short", day: "numeric", year: "numeric" })}`;
}