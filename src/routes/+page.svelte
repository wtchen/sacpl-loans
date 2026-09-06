<!--
  The dropdown panel. This page owns all app state and async flows (login,
  refreshes, renewals, session lifecycle) and composes the UI from
  presentational components in $lib/components — each view/component file
  holds its own markup and scoped styles.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import type { Event as TauriEvent, UnlistenFn } from "@tauri-apps/api/event";
  import { withTimeout } from "$lib/withTimeout";
  import type { LibStatus, Loan, Checkouts, RenewResult } from "$lib/types";
  import ConnectingView from "$lib/components/ConnectingView.svelte";
  import LoginView from "$lib/components/LoginView.svelte";
  import AccountHeader from "$lib/components/AccountHeader.svelte";
  import ReconnectingBanner from "$lib/components/ReconnectingBanner.svelte";
  import LoanList from "$lib/components/LoanList.svelte";
  import ListState from "$lib/components/ListState.svelte";
  import RefreshFooter from "$lib/components/RefreshFooter.svelte";
  import Settings from "$lib/components/Settings.svelte";
  import Toast from "$lib/components/Toast.svelte";

  let phase: "connecting" | "login" | "account" | "settings" = "connecting";
  let busy = false;
  let loginError = "";

  let loans: Loan[] = [];
  let lastLoaded = "";
  let lastStillSyncing = false;
  let renewing: Record<string, boolean> = {};
  let toast = { msg: "", kind: "ok" as "ok" | "err" };
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  let statusPoll: ReturnType<typeof setInterval> | null = null;
  let lastFetchAt = 0; // last time a FRESH checkout list was fetched (ms epoch)
  let lastUpdated = 0; // display only: when the list on screen was fetched (incl. cache's save time)
  let hasCache = false; // the current list came from the on-disk cache
  let reconnecting = false; // a refresh is re-authenticating an expired session
  let debugActive = false;
  let debugLoanIds = new Set<string>();

  function stopStatusPoll() {
    if (statusPoll) {
      clearInterval(statusPoll);
      statusPoll = null;
    }
  }

  function showToast(msg: string, kind: "ok" | "err" = "ok") {
    toast = { msg, kind };
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = { msg: "", kind: "ok" }), 5000);
  }

  async function loadCheckouts(_silent = false) {
    busy = true;
    lastStillSyncing = false;
    try {
      const r =
        // Long budget: on an expired session the backend re-authenticates
        // with the Keychain credentials and fetches again before returning.
        (await withTimeout(invoke<Checkouts>("lib_checkouts"), 150000, "Loading your loans")) ??
        undefined;
      if (r?.success) {
        loans = r.items ?? [];
        lastLoaded = r.lastLoaded ?? "";
        lastStillSyncing = !!r.stillSyncing;
        lastFetchAt = Date.now();
        lastUpdated = Date.now();
        hasCache = false;
        reconnecting = false;
      } else {
        const msg = r?.message || "Could not load your checkouts.";
        if (msg.toLowerCase().includes("not logged in") || /sign in|login/i.test(msg)) {
          phase = "login";
        } else {
          showToast(msg, "err");
        }
      }
    } catch (e) {
      reconnecting = false;
      showToast(String(e), "err");
    } finally {
      busy = false;
    }
  }

  async function checkStatus(): Promise<LibStatus | null> {
    try {
      return await withTimeout(invoke<LibStatus>("lib_status"), 15000, "Connecting to the catalog");
    } catch {
      return null;
    }
  }

  /**
   * Background refreshes are driven by a Rust thread (so they run even while
   * the panel is hidden) and arrive here as "checkouts-updated" events. Apply
   * them unless we're on the login screen or the payload is a transient
   * ILS-sync placeholder (never blank out a good list in the background).
   *
   * Session lifecycle events: when a refresh finds the catalog session
   * expired, the backend tries to sign back in with the stored credentials —
   * "session-reconnecting" makes that visible (banner + renewals disabled),
   * and "session-expired" means recovery failed: toast + login screen.
   */
  onMount(() => {
    const unlisten: Array<() => void> = [];
    listen<Loan>("debug-add-loan", (event: TauriEvent<Loan>) => {
      addDebugLoan(event.payload);
    }).then((u: UnlistenFn) => unlisten.push(u));
    listen("debug-activate", () => {
      enterDebugMode();
      phase = "account";
    }).then((u: UnlistenFn) => unlisten.push(u));
    listen<Checkouts>("checkouts-updated", (event: TauriEvent<Checkouts>) => {
      const r = event.payload;
      if (phase === "login" || !r?.success) return;
      if (r.stillSyncing && !r.items?.length) return;
      loans = r.items ?? [];
      lastLoaded = r.lastLoaded ?? "";
      lastStillSyncing = false;
      lastFetchAt = Date.now();
      lastUpdated = Date.now();
      hasCache = false;
      reconnecting = false;
    }).then((u: UnlistenFn) => unlisten.push(u));
    listen("session-reconnecting", () => {
      if (phase !== "account" && phase !== "settings") return;
      reconnecting = true;
    }).then((u: UnlistenFn) => unlisten.push(u));
    listen("session-reconnected", () => {
      reconnecting = false;
    }).then((u: UnlistenFn) => unlisten.push(u));
    listen("session-expired", () => {
      // Only a surprise when we believed we were signed in (e.g. a
      // deliberate Sign Out already shows the login form).
      if (phase !== "account" && phase !== "settings") return;
      reconnecting = false;
      stopStatusPoll();
      showToast("Your library session ended — please sign in again.", "err");
      phase = "login";
    }).then((u: UnlistenFn) => unlisten.push(u));
    // Safety net: a hidden webview can be suspended and miss events, so when
    // the panel opens with data older than 30 minutes, refresh silently.
    listen("panel-shown", () => {
      if (phase === "account" && !busy && Date.now() - lastFetchAt > 30 * 60 * 1000) {
        void loadCheckouts(true);
      }
    }).then((u: UnlistenFn) => unlisten.push(u));
    void init();
    return () => {
      stopStatusPoll();
      for (const u of unlisten) u();
    };
  });

  async function init() {
    // 1) Instant paint from the on-disk cache (written after every successful
    //    fetch), then reconcile/refresh against the catalog below.
    try {
      const c = (await withTimeout(
        invoke<Checkouts | null>("lib_cached_checkouts"),
        5000,
        "Reading saved list"
      )) ?? null;
      if (c?.success && c.items?.length) {
        loans = c.items;
        lastLoaded = c.lastLoaded ?? "";
        lastUpdated = c.savedAt ? c.savedAt * 1000 : 0;
        hasCache = true;
        phase = "account"; // provisional — reconciled against the catalog below
      }
    } catch {
      // The cache is best-effort; the normal flow continues without it.
    }

    let s = await checkStatus();
    // The bridge webview needs a few seconds to load the catalog on first
    // launch (and to pass any Cloudflare challenge). Poll until it reports.
    for (let i = 0; i < 45 && (!s || !s.ready); i++) {
      await new Promise((r) => setTimeout(r, 2000));
      s = await checkStatus();
    }
    if (!s || !s.ready) {
      showToast(
        hasCache
          ? "Could not reach the catalog — showing your saved list."
          : "Still connecting to the catalog…",
        "err"
      );
      if (!hasCache) phase = "login";
      return;
    }
    if (s.loggedIn) {
      phase = "account";
      loadCheckouts(true);
      return;
    }
    // Ready but logged out: the backend's silent Keychain re-login may still
    // be in flight. Show the form unless a cached list is on screen; give the
    // re-login up to 90s before falling back to the form for good.
    if (!hasCache) phase = "login";
    if (s.loginError) loginError = s.loginError;
    let waited = 0;
    statusPoll = setInterval(async () => {
      const s2 = await checkStatus();
      if (s2?.loggedIn) {
        stopStatusPoll();
        phase = "account";
        await loadCheckouts(true);
        return;
      }
      waited += 3;
      if (waited >= 90) {
        stopStatusPoll();
        phase = "login";
      }
    }, 3000);
  }

  async function doLogin(username: string, password: string) {
    // LoginView validates and trims; this is just a defensive guard.
    if (!username || !password) return;
    busy = true;
    loginError = "";
    try {
      const r = await withTimeout(
        invoke<{ ok: boolean; message: string }>("lib_login", { username, password }),
        120000,
        "Signing in"
      );
      if (r.ok) {
        phase = "account";
        await loadCheckouts(true);
      } else {
        loginError = r.message || "Sign in failed. Check your library ID and PIN.";
      }
    } catch (err) {
      loginError = String(err);
    } finally {
      busy = false;
    }
  }

  async function doLogout() {
    busy = true;
    stopStatusPoll();
    try {
      await withTimeout(invoke("lib_logout"), 60000, "Signing out");
    } finally {
      loans = [];
      lastLoaded = "";
      lastUpdated = 0;
      hasCache = false;
      reconnecting = false;
      phase = "login";
      busy = false;
    }
  }

  async function quitApp() {
    try {
      await invoke("lib_quit");
    } catch {
      // Quitting anyway — nothing to report.
    }
  }

  // Called by the Settings component after it clears the on-disk cache: back
  // to the account view and reload from the catalog.
  function cacheCleared() {
    hasCache = false;
    phase = "account";
    void loadCheckouts(true);
  }

  function enterDebugMode() {
    debugActive = true;
    showToast("Debug mode is on. Add temporary books from Debug Events.");
  }

  function addDebugLoan(loan: Loan) {
    debugLoanIds = new Set([...debugLoanIds, loan.recordId]);
    loans = [...loans, loan];
  }

  function exitDebugMode() {
    loans = loans.filter((loan) => !debugLoanIds.has(loan.recordId));
    debugLoanIds = new Set();
    debugActive = false;
    void invoke("lib_show_debug_events", { show: false });
    showToast("Debug mode is off. Temporary books were removed.");
  }

  function libbyNote() {
    showToast("Libby books have to be renewed through the Libby app.");
  }

  async function openLoan(item: Loan) {
    if (!item.url) return;
    try {
      await invoke("lib_open_url", { url: item.url });
    } catch (e) {
      showToast(String(e), "err");
    }
  }

  async function renewOne(item: Loan) {
    if (renewBlocked()) return;
    renewing = { ...renewing, [item.recordId]: true };
    try {
      // Fake debug loans never call the catalog. Simulate a successful renewal
      // by moving their due date three weeks forward in the panel only.
      if (debugLoanIds.has(item.recordId)) {
        const due = new Date();
        due.setDate(due.getDate() + 21);
        loans = loans.map((loan) =>
          loan.recordId === item.recordId
            ? { ...loan, due: `Due ${due.toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" })}` }
            : loan
        );
        showToast("Renewed! (simulated)");
        return;
      }
      const r = await withTimeout(
        invoke<RenewResult>("lib_renew_one", {
          kind: item.kind,
          patronId: item.patronId,
          recordId: item.recordId,
          renewIndicator: item.renewIndicator,
        }),
        120000,
        "Renewing"
      );
      showToast(
        r.success ? r.message || "Renewed!" : r.message || r.title || "Renewal failed.",
        r.success ? "ok" : "err"
      );
      if (r.success) await loadCheckouts(true);
    } catch (e) {
      showToast(String(e), "err");
    } finally {
      const next = { ...renewing };
      delete next[item.recordId];
      renewing = next;
    }
  }

  const overdueCount = () => loans.filter((l) => l.overdue).length;
  // The catalog's checkoutInfoLastLoaded rarely changes between fetches (it's
  // the ILS sync time), so the footer shows OUR last successful refresh.
  const updatedLabel = () =>
    lastUpdated
      ? `Updated ${new Date(lastUpdated).toLocaleTimeString([], {
          hour: "numeric",
          minute: "2-digit",
          second: "2-digit",
        })}`
      : "";
  // Renewal is disabled while a refresh is in flight, while the list on
  // screen is only the disk cache (we don't know we're logged in yet), and
  // while the backend is signing back in after a session expiry.
  const renewBlocked = () => busy || reconnecting || (hasCache && lastFetchAt === 0);
</script>

<div class="panel">
  {#if phase === "connecting"}
    <ConnectingView onquit={quitApp} />
  {/if}

  {#if phase === "login"}
    <LoginView busy={busy} loginError={loginError} onlogin={doLogin} onquit={quitApp} />
  {/if}

  {#if phase === "account"}
    <AccountHeader
      loanCount={loans.length}
      overdueCount={overdueCount()}
      busy={busy}
      onsettings={() => (phase = "settings")} 
      onlogout={doLogout}
      onquit={quitApp}
    />

    {#if reconnecting}
      <ReconnectingBanner />
    {/if}

    {#if busy && loans.length === 0}
      <ListState kind="loading" />
    {:else if loans.length > 0}
      <LoanList
        loans={loans}
        dimmed={busy || reconnecting}
        renewDisabled={renewBlocked()}
        renewing={renewing}
        onrenew={renewOne}
        onopen={openLoan}
        onlibby={libbyNote}
      />
    {:else if lastStillSyncing}
      <ListState kind="syncing" onretry={() => loadCheckouts()} />
    {:else}
      <ListState kind="empty" />
    {/if}

    {#if !(busy && loans.length === 0)}
      <RefreshFooter
        busy={busy}
        reconnecting={reconnecting}
        updated={updatedLabel()}
        onrefresh={() => loadCheckouts()}
      />
    {/if}

    {#if debugActive}
      <div class="debug-actions">
        <button class="btn ghost" onclick={() => invoke("lib_show_debug_events", { show: true })}>Return to Events</button>
        <button class="btn ghost" onclick={exitDebugMode}>Exit Debug</button>
      </div>
    {/if}
  {/if}

  {#if phase === "settings"}
    <Settings
      onclose={() => (phase = "account")}
      oncleared={cacheCleared}
      notify={showToast}
    />
  {/if}

  <Toast msg={toast.msg} kind={toast.kind} />
</div>

<style>
  .debug-actions {
    display: flex;
    justify-content: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .debug-actions .btn {
    font-size: 11px;
    padding: 4px 10px;
  }

  .panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
    height: 100vh;
    padding: 14px;
    box-sizing: border-box;
    font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif;
    color: var(--text);
    background: var(--bg-panel);
    border-radius: 12px;
    border: 1px solid var(--border-panel);
    backdrop-filter: blur(8px);
    overflow: hidden;
  }
</style>