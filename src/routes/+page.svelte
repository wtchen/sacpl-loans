<!--
  The dropdown panel. Owns all app state and async flows (login, refreshes,
  renewals, session lifecycle); views/components live in $lib/components.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import type { Event as TauriEvent, UnlistenFn } from "@tauri-apps/api/event";
  import { withTimeout } from "$lib/withTimeout";
  import type { LibStatus, Loan, Checkouts, RenewResult, AppSettings } from "$lib/types";
  import { updatedLabel as updatedLabelText, fullStamp, isStale } from "$lib/updated-label";
  import { isRenewBlocked, simulatedRenewalDue } from "$lib/renew";
  import ConnectingView from "$lib/components/ConnectingView.svelte";
  import LoginView from "$lib/components/LoginView.svelte";
  import AccountHeader from "$lib/components/AccountHeader.svelte";
  import ReconnectingBanner from "$lib/components/ReconnectingBanner.svelte";
  import LoanList from "$lib/components/LoanList.svelte";
  import ListState from "$lib/components/ListState.svelte";
  import RefreshFooter from "$lib/components/RefreshFooter.svelte";
  import RenewConfirm from "$lib/components/RenewConfirm.svelte";
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
  let lastFetchAt = 0; // ms epoch of the last FRESH fetch
  let lastUpdated = 0; // display only: when the list on screen was fetched (incl. cache's save time)
  let hasCache = false; // the current list came from the on-disk cache
  let reconnecting = false; // a refresh is re-authenticating an expired session
  let debugActive = false;
  let debugLoanIds = new Set<string>();
  let renewConfirm: Loan | null = null; // item awaiting renewal confirmation
  let renewConfirming = false;

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
        // Long budget: an expired session is re-authenticated (Keychain)
        // and refetched before this resolves.
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

  // Background refreshes arrive from the Rust loop as "checkouts-updated"
  // events; apply them unless on the login screen or the payload is a
  // transient ILS-sync placeholder (never blank a good list in the
  // background). session-reconnecting = re-auth in flight; session-expired =
  // recovery failed.
  onMount(() => {
    const unlisten: Array<() => void> = [];
    listen<Loan>("debug-add-loan", (event: TauriEvent<Loan>) => {
      addDebugLoan(event.payload);
    }).then((u: UnlistenFn) => unlisten.push(u));
    listen<number>("debug-last-updated", (event: TauriEvent<number>) => {
      // Debug tool: pretend the list was refreshed at this time.
      lastUpdated = event.payload;
      lastFetchAt = event.payload;
      hasCache = false;
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
      // Only a surprise when we believed we were signed in; a deliberate
      // Sign Out already shows the login form.
      if (phase !== "account" && phase !== "settings") return;
      reconnecting = false;
      stopStatusPoll();
      showToast("Your library session ended — please sign in again.", "err");
      phase = "login";
    }).then((u: UnlistenFn) => unlisten.push(u));
    // On panel open, refresh data older than the refresh interval.
    listen("panel-shown", () => {
      if (phase === "account" && !busy) {
        void refreshIfStale();
      }
    }).then((u: UnlistenFn) => unlisten.push(u));
    void init();
    return () => {
      stopStatusPoll();
      for (const u of unlisten) u();
    };
  });

  // The configured refresh interval in ms (1 hour when settings are unreadable).
  async function refreshIntervalMs(): Promise<number> {
    try {
      const s = await withTimeout(invoke<AppSettings>("lib_get_settings"), 5000, "Loading settings");
      return (s?.refreshSecs ?? 3600) * 1000;
    } catch {
      return 60 * 60 * 1000;
    }
  }

  /** Refresh when the list on screen is older than the refresh interval. */
  async function refreshIfStale() {
    if (busy || phase === "login") return;
    if (!lastUpdated) {
      await loadCheckouts(true);
      return;
    }
    if (isStale(lastUpdated, Date.now(), await refreshIntervalMs())) {
      await loadCheckouts(true);
    } else if (hasCache && lastFetchAt === 0) {
      // Cache is within the interval — trust it for renewals too.
      lastFetchAt = lastUpdated;
    }
  }

  async function init() {
    // Paint the on-disk cache instantly, then reconcile with the catalog.
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
        phase = "account"; // provisional until the catalog check below
      }
    } catch {
      // Cache is best-effort; continue without it.
    }

    let s = await checkStatus();
    // First launch needs a few seconds to load the catalog and pass any
    // Cloudflare challenge — poll until the bridge reports ready.
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
      await refreshIfStale();
      return;
    }
    // Ready but logged out: the silent Keychain re-login may still be in
    // flight; give it up to 90s before falling back to the login form.
    if (!hasCache) phase = "login";
    if (s.loginError) loginError = s.loginError;
    let waited = 0;
    statusPoll = setInterval(async () => {
      const s2 = await checkStatus();
      if (s2?.loggedIn) {
        stopStatusPoll();
        phase = "account";
        await refreshIfStale();
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

  // Called by Settings after it clears the on-disk cache.
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

  function askRenew(item: Loan) {
    if (renewBlocked) return;
    renewConfirm = item;
  }

  /** Renewal runs with the dialog open (spinner) until the result lands. */
  async function confirmRenew() {
    const item = renewConfirm;
    if (!item || renewConfirming) return;
    renewConfirming = true;
    try {
      await doRenew(item);
    } finally {
      renewConfirming = false;
      renewConfirm = null;
    }
  }

  async function doRenew(item: Loan) {
    if (renewBlocked) return;
    renewing = { ...renewing, [item.recordId]: true };
    try {
      // Fake debug loans never call the catalog: +3 weeks, panel only.
      if (debugLoanIds.has(item.recordId)) {
        const dueLabel = simulatedRenewalDue(Date.now());
        loans = loans.map((loan) =>
          loan.recordId === item.recordId ? { ...loan, due: dueLabel } : loan
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

  // Reactive declarations, NOT const helpers called in the template —
  // Svelte only tracks variables referenced directly in template
  // expressions, so a const function reading `lastUpdated` never updates.
  $: overdueCount = loans.filter((l) => l.overdue).length;
  $: updatedLabel = updatedLabelText(lastUpdated, Date.now());
  $: updatedTitle = lastUpdated ? fullStamp(lastUpdated, Date.now()) : "";
  $: renewBlocked = isRenewBlocked({ busy, reconnecting, hasCache, lastFetchAt });
</script>

<svelte:window
  onkeydown={(e) => {
    if (renewConfirm && e.key === "Escape") renewConfirm = null;
  }}
/>
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
      overdueCount={overdueCount}
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
        renewDisabled={renewBlocked}
        renewing={renewing}
        onrenew={askRenew}
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
        updated={updatedLabel}
        updatedTitle={updatedTitle}
        onrefresh={() => loadCheckouts()}
      />
    {/if}

    {#if debugActive}
      <div class="debug-actions">
        <button class="btn ghost" onclick={() => invoke("lib_show_debug_events", { show: true })}>Return to Events</button>
        <button class="btn ghost" onclick={exitDebugMode}>Exit Debug</button>
      </div>
    {/if}

    {#if renewConfirm}
      <RenewConfirm
        item={renewConfirm}
        busy={renewConfirming}
        oncancel={() => (renewConfirm = null)}
        onconfirm={confirmRenew}
      />
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