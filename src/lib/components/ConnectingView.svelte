<!--
  The "connecting" phase: shown while the hidden bridge webview loads the
  catalog (and passes any Cloudflare challenge).
-->
<script lang="ts">
  /** Quit the whole app. */
  export let onquit: () => void;
</script>

<div class="state">
  <div class="spinner" aria-hidden="true"></div>
  <p>Connecting to the catalog…</p>
  <button class="btn ghost quit-link" onclick={onquit}>Quit</button>
</div>

<style>
  .state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: var(--text-2);
    font-size: 13px;
  }
</style>