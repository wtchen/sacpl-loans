<!--
  Settings view. Owns the refresh-interval, cache-clear, and developer
  actions; the parent view owns toasts, phase switching, and the catalog
  reload that follows a cache clear (via the callbacks below).
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import type { Event as TauriEvent, UnlistenFn } from "@tauri-apps/api/event";
  import { withTimeout } from "$lib/withTimeout";
  import type { AppSettings } from "$lib/types";

  /** Called when the user taps Done (close the settings view). */
  export let onclose: () => void;
  /**
   * Called after the on-disk checkout cache has been cleared, so the parent
   * can switch back to the account view and reload from the catalog.
   */
  export let oncleared: () => void;
  /** Surface a toast in the parent view. */
  export let notify: (msg: string, kind?: "ok" | "err") => void;
  let view: "main" | "dev" | "log" = "main";
  let refreshMinutes = 60;
  let settingsBusy = false;
  let bridgeVisible = false;
  let bridgeBusy = false;
  let debugMode = false; // hidden entirely in default (non-debug) builds
  let logLines: string[] = [];
  let logBusy = false;
  let logBox: HTMLElement | undefined;

  onMount(() => {
    const unlisten: Array<() => void> = [];
    // Auto-detect the browser window's state: Rust emits this whenever the
    // window is shown, hidden, or closed (close only hides it).
    listen<boolean>("bridge-visibility", (event: TauriEvent<boolean>) => {
      bridgeVisible = !!event.payload;
    }).then((u: UnlistenFn) => unlisten.push(u));
    // The panel usually hides (and can be App-Nap suspended) while the
    // browser window is open, so events can be missed — re-sync the real
    // state whenever the panel is shown again. The log view also re-reads
    // the file to pick up lines logged while the panel was hidden.
    listen("panel-shown", () => {
      void syncBridgeVisible();
      if (view === "log") void loadLog();
    }).then((u: UnlistenFn) => unlisten.push(u));
    // Live log lines appended while the viewer is open.
    listen<string>("log-appended", (event: TauriEvent<string>) => {
      if (view !== "log") return;
      logLines = [...logLines, event.payload].slice(-500);
      requestAnimationFrame(() => logBox?.scrollTo({ top: logBox.scrollHeight }));
    }).then((u: UnlistenFn) => unlisten.push(u));
    void loadSettings();
    return () => {
      for (const u of unlisten) u();
    };
  });

  async function loadSettings() {
    try {
      const s = await withTimeout(
        invoke<AppSettings>("lib_get_settings"),
        10000,
        "Loading settings"
      );
      refreshMinutes = Math.max(10, Math.round((s?.refreshSecs ?? 3600) / 60));
      bridgeVisible = !!s?.bridgeVisible;
      debugMode = !!s?.debugMode;
    } catch {
      // Keep the defaults; saving will still work.
    }
  }

  /** Re-read the window's actual visibility (cheap, idempotent). */
  async function syncBridgeVisible() {
    try {
      const s = await withTimeout(
        invoke<AppSettings>("lib_get_settings"),
        10000,
        "Loading settings"
      );
      bridgeVisible = !!s?.bridgeVisible;
    } catch {
      // Keep the current state; the next event or refresh will correct it.
    }
  }

  async function saveRefreshInterval() {
    settingsBusy = true;
    try {
      const mins = Math.max(10, Math.round(refreshMinutes || 60));
      const r = await withTimeout(
        invoke<{ refreshSecs: number }>("lib_set_refresh_secs", { secs: mins * 60 }),
        10000,
        "Saving settings"
      );
      refreshMinutes = Math.max(10, Math.round(r.refreshSecs / 60));
      notify(
        `Auto-refresh set to every ${refreshMinutes === 60 ? "hour" : `${refreshMinutes} minutes`}.`
      );
    } catch (e) {
      notify(String(e), "err");
    } finally {
      settingsBusy = false;
    }
  }

  async function clearCacheAndReload() {
    settingsBusy = true;
    try {
      await withTimeout(invoke("lib_clear_cache"), 10000, "Clearing saved list");
      notify("Saved list cleared — reloading…");
      oncleared();
    } catch (e) {
      notify(String(e), "err");
    } finally {
      settingsBusy = false;
    }
  }

  async function toggleBridge() {
    bridgeBusy = true;
    try {
      const visible = await withTimeout(
        invoke<boolean>("lib_show_bridge", { show: !bridgeVisible }),
        10000,
        "Toggling the hidden browser"
      );
      bridgeVisible = !!visible;
    } catch (e) {
      notify(String(e), "err");
    } finally {
      bridgeBusy = false;
    }
  }

  async function resetBridge() {
    bridgeBusy = true;
    try {
      await withTimeout(invoke("lib_reset_bridge"), 10000, "Resetting the hidden browser");
      notify("The hidden browser is back on the catalog page.");
    } catch (e) {
      notify(String(e), "err");
    } finally {
      bridgeBusy = false;
    }
  }

  async function openDebugEvents() {
    try {
      await withTimeout(invoke("lib_show_debug_events", { show: true }), 10000, "Opening Debug Events");
    } catch (e) {
      notify(String(e), "err");
    }
  }

  async function loadLog() {
    logBusy = true;
    try {
      const text = await withTimeout(
        invoke<string>("lib_read_log"),
        10000,
        "Reading the app log"
      );
      logLines = text ? text.split("\n").filter((l) => l.trim()).slice(-500) : [];
      requestAnimationFrame(() => logBox?.scrollTo({ top: logBox.scrollHeight }));
    } catch (e) {
      notify(String(e), "err");
    } finally {
      logBusy = false;
    }
  }
</script>

<header class="head row">
  <div>
    <h1>
      {view === "dev" ? "Developer Settings" : view === "log" ? "App Log" : "Settings"}
    </h1>
    <p class="sub">SacPL Loans</p>
  </div>
  {#if view === "dev" || view === "log"}
    <button class="btn ghost" onclick={() => (view = view === "dev" ? "main" : "dev")}>Back</button>
  {:else}
    <button class="btn ghost" onclick={onclose}>Done</button>
  {/if}
</header>

{#if view === "main"}
  <div class="settings">
    <label class="field">
      <span>Auto-refresh interval</span>
      <select
        class="select"
        bind:value={refreshMinutes}
        onchange={saveRefreshInterval}
        disabled={settingsBusy}
      >
        <option value={10}>Every 10 minutes</option>
        <option value={15}>Every 15 minutes</option>
        <option value={30}>Every 30 minutes</option>
        <option value={60}>Every hour</option>
        <option value={120}>Every 2 hours</option>
        <option value={240}>Every 4 hours</option>
      </select>
    </label>
    <p class="hint">
      The loan list refreshes in the background, even while this panel is closed. The shortest
      interval is 10 minutes.
    </p>

    <button class="btn" onclick={clearCacheAndReload} disabled={settingsBusy}>
      {settingsBusy ? "Working…" : "Clear Saved List"}
    </button>
    <p class="hint">
      Deletes the cached checkout list stored on this Mac and reloads it from the catalog right
      away.
    </p>

    {#if debugMode}
      <button class="btn ghost dev-link" onclick={() => (view = "dev")}>View Developer Settings</button>
    {/if}
  </div>
{:else if view === "dev"}
  <div class="settings">
    <p class="hint">
      The app reaches catalog.saclibrary.org through a hidden WebKit browser — the “bridge” —
      which passes Cloudflare checks and holds the sign-in session. Opening it shows exactly what
      the app sees; signing in or out there changes the app's session too.
    </p>
    <button class="btn" onclick={toggleBridge} disabled={bridgeBusy}>
      {bridgeBusy ? "Working…" : bridgeVisible ? "Hide the Hidden Browser" : "Open the Hidden Browser"}
    </button>
    <p class="hint">
      Closing the window only hides it — the app keeps using it either way. Opening it returns to
      the catalog page if it was navigated elsewhere.
    </p>
    <button class="btn ghost" onclick={resetBridge} disabled={bridgeBusy}>
      Reload the Catalog Page
    </button>

    <button class="btn ghost dev-link" onclick={() => { view = "log"; void loadLog(); }}>
      View App Log
    </button>
    <button class="btn ghost dev-link" onclick={openDebugEvents}>Open Debug Events Window</button>
  </div>
{:else if view === "log"}
  <div class="log-wrap">
    <pre class="log-box" bind:this={logBox}>{logLines.length ? logLines.join("\n") : logBusy ? "Loading…" : "No events logged yet."}</pre>
  </div>
{/if}

<style>
  .settings {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 6px;
    overflow-y: auto;
  }
  .settings .btn {
    align-self: flex-start;
  }
  .settings .dev-link {
    align-self: center;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-2);
    padding: 4px 10px;
  }
  .select {
    background: var(--input-bg);
    border: 1px solid var(--input-border);
    color: var(--text);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 13px;
    outline: none;
    appearance: auto;
  }
  .select:focus {
    border-color: var(--accent);
  }

  .log-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    margin-top: 6px;
  }
  .log-box {
    flex: 1;
    margin: 0;
    overflow-y: auto;
    background: var(--log-bg);
    border: 1px solid var(--row-border);
    border-radius: 8px;
    padding: 8px 10px;
    box-sizing: border-box;
    font-family: "SF Mono", ui-monospace, Menlo, monospace;
    font-size: 10.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--log-text);
  }
</style>