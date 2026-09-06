<!--
  The sign-in form for the catalog. Owns the input fields and the
  empty-fields validation; the parent runs the actual sign-in command.
-->
<script lang="ts">
  /** A sign-in is in flight; disable the form. */
  export let busy: boolean;
  /** Server-provided sign-in error, if any. */
  export let loginError: string;
  /** Submit credentials; the page runs the actual command. */
  export let onlogin: (username: string, password: string) => void;
  /** Quit the whole app. */
  export let onquit: () => void;

  let username = "";
  let password = "";
  let formError = "";

  function submit(e: Event) {
    e.preventDefault();
    if (!username.trim() || !password) {
      formError = "Enter your library ID and PIN.";
      return;
    }
    formError = "";
    onlogin(username.trim(), password);
  }
</script>

<header class="head">
  <h1>Sacramento Public Library</h1>
  <p class="sub">Sign in to see your loans</p>
</header>

<form onsubmit={submit} class="login">
  <label class="field">
    <span>Library ID / Username</span>
    <input
      type="text"
      bind:value={username}
      autocomplete="username"
      autocapitalize="off"
      spellcheck="false"
      placeholder="e.g. 1234567890"
      disabled={busy}
    />
  </label>
  <label class="field">
    <span>PIN / Password</span>
    <input
      type="password"
      bind:value={password}
      autocomplete="current-password"
      placeholder="Your library PIN"
      disabled={busy}
    />
  </label>
  {#if loginError || formError}
    <p class="error" role="alert">{loginError || formError}</p>
  {/if}
  <button type="submit" class="btn primary wide" disabled={busy}>
    {busy ? "Signing in…" : "Sign In"}
  </button>
  <p class="hint">Your session is saved on this Mac and restored when the app restarts.</p>
  <button type="button" class="btn ghost quit-link" onclick={onquit}>Quit</button>
</form>

<style>
  .login {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 6px;
  }
  .error {
    margin: 0;
    font-size: 12px;
    color: var(--danger);
  }
</style>