<!-- Shown while the backend re-authenticates an expired catalog session. -->
<div class="reconnecting">
  <span class="mini-spinner" aria-hidden="true"></span>
  <span>Your library session ended — signing you back in…</span>
</div>

<style>
  .reconnecting {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-2);
    background: var(--row-bg);
    border: 1px solid var(--row-border);
    border-radius: 10px;
    padding: 8px 10px;
  }
</style>