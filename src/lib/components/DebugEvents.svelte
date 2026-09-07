<!-- Runtime-only debug tools. Fake loans are kept in the panel state and never reach the catalog or cache. -->
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { Loan } from "$lib/types";
  import { withTimeout } from "$lib/withTimeout";

  export let debugActive: boolean;
  export let onactivate: () => void;
  export let onadd: (loan: Loan) => void;

  type FakeBook = { label: string; detail: string; libby?: boolean; renewable?: boolean; days: number };

  let real = false;
  let firing = false;

  type DebugEvent = { id: string; label: string; detail: string; realPossible: boolean };
  const EVENTS: DebugEvent[] = [
    { id: "http403", label: "Catalog returns 403", detail: "Cloudflare block — the next refresh fails with HTTP 403.", realPossible: false },
    { id: "http500", label: "Catalog returns 500", detail: "Server error — the next refresh fails with HTTP 500.", realPossible: false },
    { id: "unreachable", label: "Catalog unreachable", detail: "Network failure — the next refresh errors out.", realPossible: false },
    { id: "syncing", label: "ILS still syncing", detail: "The next refresh returns the empty still-syncing placeholder.", realPossible: false },
    { id: "changedData", label: "Loans changed server-side", detail: "The next refresh returns a simulated two-book list.", realPossible: true },
    { id: "loggedOut", label: "Session expired (recoverable)", detail: "The next refresh reports the session ended; the app signs back in from the Keychain.", realPossible: true },
    { id: "expiredUnrecoverable", label: "Session expired (unrecoverable)", detail: "Reconnect banner, then the sign-in screen — your saved sign-in is kept.", realPossible: false },
  ];

  const BOOK_TYPES: FakeBook[] = [
    { label: "Add Libby book", detail: "An ebook checked out through Libby.", libby: true, days: 14 },
    { label: "Add renewable book · due in 2 weeks", detail: "A physical item with a normal renewal available.", renewable: true, days: 14 },
    { label: "Add renewable book · due tomorrow", detail: "A physical item ready to renew soon.", renewable: true, days: 1 },
    { label: "Add max-renewals book", detail: "A physical item that cannot be renewed again.", days: 1 },
  ];

  /** Pretend the list was last refreshed at these offsets (ms ago). */
  const REFRESH_SCENARIOS: { label: string; detail: string; agoMs: () => number }[] = [
    { label: "Refreshed just now", detail: "The footer shows today's time.", agoMs: () => 0 },
    { label: "3 hours ago", detail: "Older than the default 1-hour interval — reopening refreshes.", agoMs: () => 3 * 60 * 60 * 1000 },
    { label: "Yesterday", detail: "The footer shows “Updated yesterday”.", agoMs: () => 24 * 60 * 60 * 1000 },
    { label: "3 days ago", detail: "The footer shows “Updated 3 days ago”.", agoMs: () => 3 * 24 * 60 * 60 * 1000 },
    { label: "20 days ago", detail: "The footer shows the date, e.g. “Updated 17 Aug 2026”.", agoMs: () => 20 * 24 * 60 * 60 * 1000 },
  ];

  async function setRefreshTime(scenario: { label: string; agoMs: () => number }) {
    try {
      await invoke("lib_debug_set_refresh_time", { atMs: Date.now() - scenario.agoMs() });
    } catch (error) {
      console.error("Setting refresh time failed", error);
    }
  }

  const ADJECTIVES = ["Last", "Hidden", "Quiet", "Golden", "Forgotten", "Northern", "Midnight", "Small"];
  const NOUNS = ["Garden", "Map", "Harbor", "Library", "Orchard", "River", "House", "Archive"];
  const SUBTITLES = ["a novel", "stories", "a memoir", "an investigation", "notes from the coast"];

  function randomTitle() {
    const pick = <T,>(items: T[]) => items[Math.floor(Math.random() * items.length)];
    return `The ${pick(ADJECTIVES)} ${pick(NOUNS)}: ${pick(SUBTITLES)}`;
  }

  async function fire(event: DebugEvent) {
    if (firing) return;
    firing = true;
    try {
      await withTimeout(invoke<string>("lib_debug_event", { event: event.id, real }), 20000, "Triggering debug event");
    } catch (error) {
      console.error("Debug event failed", error);
    } finally {
      firing = false;
    }
  }

  function addBook(type: FakeBook) {
    const due = new Date();
    due.setDate(due.getDate() + type.days);
    const id = `debug-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    onadd({
      title: randomTitle(),
      due: `Due ${due.toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" })}`,
      libby: type.libby,
      overdue: false,
      kind: type.libby ? "overdrive" : "ils",
      patronId: "debug-patron",
      recordId: id,
      renewIndicator: type.renewable ? "debug-renewable" : "",
      renewable: !!type.renewable,
    });
  }
</script>

<div class="debug">
  {#if !debugActive}
    <p class="hint">Debug mode adds temporary fake loans to this panel. It never changes your catalog or saved list.</p>
    <button class="btn" onclick={onactivate}>Enter Debug Mode</button>
  {:else}
    <p class="hint">Debug mode is on. Fake loans are temporary and disappear when you exit debug mode.</p>

    <section>
      <h2>Fake Books</h2>
      <div class="events">
        {#each BOOK_TYPES as type (type.label)}
          <div class="event">
            <button class="btn" onclick={() => addBook(type)} title={type.detail}>{type.label}</button>
            <span class="event-detail">{type.detail}</span>
          </div>
        {/each}
      </div>
    </section>

    <section>
      <h2>Refresh Time</h2>
      <div class="events">
        {#each REFRESH_SCENARIOS as scenario (scenario.label)}
          <div class="event">
            <button class="btn" onclick={() => setRefreshTime(scenario)} title={scenario.detail}>{scenario.label}</button>
            <span class="event-detail">{scenario.detail}</span>
          </div>
        {/each}
      </div>
    </section>

    <section>
      <h2>Catalog Events</h2>
      <label class="real-toggle">
        <input type="checkbox" bind:checked={real} />
        <span>Trigger the <strong>real</strong> event on the site (when possible)</span>
      </label>
      <div class="events">
        {#each EVENTS as event (event.id)}
          <div class="event">
            <button class="btn" onclick={() => fire(event)} disabled={firing || (real && !event.realPossible)} title={event.detail}>{event.label}</button>
            <span class="event-detail">{event.detail}</span>
          </div>
        {/each}
      </div>
    </section>
  {/if}
</div>

<style>
  .debug { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 10px; margin-top: 6px; overflow-y: auto; }
  section { display: flex; flex-direction: column; gap: 8px; }
  h2 { margin: 4px 0 0; font-size: 13px; }
  .events { display: flex; flex-direction: column; gap: 12px; }
  .real-toggle { display: flex; gap: 8px; align-items: flex-start; font-size: 12px; color: var(--text); cursor: pointer; }
  .real-toggle input { margin-top: 2px; }
  .event { display: flex; flex-direction: column; gap: 3px; }
  .event .btn { align-self: flex-start; }
  .event-detail { font-size: 11px; color: var(--text-3); line-height: 1.4; }
</style>
