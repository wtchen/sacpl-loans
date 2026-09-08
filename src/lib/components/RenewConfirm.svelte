<!-- Renewal confirmation dialog: backdrop click, Cancel or Esc dismisses;
     Renew runs the renewal while the dialog stays open. -->
<script lang="ts">
  import type { Loan } from "$lib/types";

  export let item: Loan;
  /** True while the renewal request is in flight (buttons lock, "…"). */
  export let busy: boolean;
  export let oncancel: () => void;
  export let onconfirm: () => void;
</script>

<div class="backdrop" role="presentation" onmousedown={oncancel}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="renew-confirm-title"
    tabindex="-1"
    onmousedown={(e) => e.stopPropagation()}
  >
    <h2 id="renew-confirm-title">Renew this item?</h2>
    <p class="title">{item.title || "Untitled item"}</p>
    <p class="hint">Renewing extends the loan to the library's next available due date.</p>
    <div class="row">
      <button class="btn ghost" onclick={oncancel} disabled={busy}>Cancel</button>
      <button class="btn" onclick={onconfirm} disabled={busy}>{busy ? "Renewing…" : "Renew"}</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: color-mix(in srgb, var(--text) 35%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
  }
  .dialog {
    width: 100%;
    max-width: 300px;
    background: var(--bg-panel);
    border: 1px solid var(--control-border);
    border-radius: 12px;
    padding: 14px 16px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
  }
  h2 {
    margin: 0;
    font-size: 14px;
  }
  .title {
    margin: 0;
    font-size: 12.5px;
    font-weight: 650;
    color: var(--text);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .hint {
    margin: 0;
    font-size: 11.5px;
    color: var(--text-2);
    line-height: 1.45;
  }
  .row {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
</style>
