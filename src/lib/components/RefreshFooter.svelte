<!-- Footer: manual refresh plus when the list was last updated. -->
<script lang="ts">
  export let busy: boolean; // refresh in flight → spinner + disabled
  export let reconnecting: boolean; // session restore in flight → disabled
  export let updated: string; // preformatted "Updated …" label ("" = never)
  export let updatedTitle = ""; // hover tooltip: full timestamp
  export let onrefresh: () => void;
</script>

<footer class="foot">
  <button class="btn ghost refresh-btn" onclick={onrefresh} disabled={busy || reconnecting}>
    {#if busy}<span class="mini-spinner" aria-hidden="true"></span>Refreshing…{:else}Refresh{/if}
  </button>
  <span class="updated" title={updatedTitle}>{updated}</span>
</footer>

<style>
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 11px;
    color: var(--text-3);
  }
  .refresh-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .updated {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>