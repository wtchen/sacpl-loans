<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { Loan } from "$lib/types";
  import DebugEvents from "$lib/components/DebugEvents.svelte";

  let debugActive = false;

  async function activate() {
    try {
      await invoke("lib_enter_debug_mode");
      debugActive = true;
    } catch (error) {
      console.error("Could not enter debug mode", error);
    }
  }

  async function addLoan(loan: Loan) {
    try {
      await invoke("lib_add_debug_loan", { loan });
    } catch (error) {
      console.error("Could not add fake loan", error);
    }
  }
</script>

<div class="panel">
  <DebugEvents {debugActive} onactivate={activate} onadd={addLoan} />
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100vh;
    padding: 14px;
    box-sizing: border-box;
    font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif;
    color: var(--text);
    background: var(--bg-panel);
  }
</style>
