<!-- The list-area placeholder states: loading, still-syncing, and empty. -->
<script lang="ts">
  export let kind: "loading" | "syncing" | "empty";
  /** Retry handler (only used by the still-syncing state). */
  export let onretry: () => void = () => {};
</script>

{#if kind === "loading"}
  <div class="list-state">
    <div class="spinner" aria-hidden="true"></div>
    <p class="list-state-msg">Loading your loans from the library…</p>
    <p class="list-state-hint">This can take up to a minute while the catalog syncs.</p>
  </div>
{:else if kind === "syncing"}
  <div class="list-state">
    <div class="spinner" aria-hidden="true"></div>
    <p class="list-state-msg">The library is still syncing your loans.</p>
    <button class="btn ghost" onclick={() => onretry()}>Try again</button>
  </div>
{:else}
  <div class="list-state">
    <p class="list-state-msg">No items checked out.</p>
  </div>
{/if}

<style>
  .list-state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    color: var(--text-2);
    font-size: 13px;
    text-align: center;
    padding: 24px 16px;
  }
  .list-state-msg {
    margin: 0;
    font-size: 13px;
  }
  .list-state-hint {
    margin: 0;
    font-size: 11px;
    color: var(--text-3);
  }
</style>