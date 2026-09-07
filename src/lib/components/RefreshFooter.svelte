<!-- Footer: manual refresh plus when the list was last updated. -->
<script lang="ts">
  /** A refresh is in flight → show the spinner and "Refreshing…". */
  export let busy: boolean;
  /** A session restore is in flight → disable (no label change). */
  export let reconnecting: boolean;
  /** Preformatted "Updated …" label ("" when never updated). */
  export let updated: string;
  /** Hover tooltip: full timestamp of the last update. */
  export let updatedTitle = "";
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