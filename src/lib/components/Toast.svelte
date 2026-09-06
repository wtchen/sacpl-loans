<!-- The transient toast overlay (owned by the page; this is display only). -->
<script lang="ts">
  export let msg: string;
  export let kind: "ok" | "err" = "ok";
</script>

{#if msg}
  <div class="toast {kind}">{msg}</div>
{/if}

<style>
  .toast {
    position: fixed;
    left: 50%;
    transform: translateX(-50%);
    bottom: 12px;
    max-width: 90%;
    font-size: 12px;
    padding: 7px 12px;
    border-radius: 8px;
    background: rgba(10, 132, 255, 0.92);
    color: white;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
  }
  .toast.err {
    background: rgba(200, 40, 40, 0.95);
  }
</style>