<!-- The account view header: loan summary plus the view actions. -->
<script lang="ts">
  export let loanCount: number;
  export let overdueCount: number;
  export let busy: boolean;
  export let onsettings: () => void;
  export let onlogout: () => void;
  export let onquit: () => void;
</script>

<header class="head row">
  <div>
    <h1>My Loans</h1>
    <p class="sub">
      {loanCount} checked out{overdueCount ? ` · ${overdueCount} overdue` : ""}
    </p>
  </div>
  <div class="head-actions">
    <button class="btn ghost" onclick={onsettings}>Settings</button>
    <button class="btn ghost" onclick={onlogout} disabled={busy}>Sign Out</button>
    <button class="btn ghost" onclick={onquit}>Quit</button>
  </div>
</header>

<style>
  .head-actions {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
  }
</style>