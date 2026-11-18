<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { operations } from './ttp'
  import type { AuthPrompt, Operation } from './ttp'

  let { prompt, operation }: { prompt: AuthPrompt; operation: Operation } = $props()
  let dialog: HTMLDialogElement
  let input: HTMLInputElement
  let sending = $state(false)
  let error = $state('')

  onMount(() => {
    dialog.showModal()
    input.focus()
    return () => dialog.close()
  })

  async function authenticate(event: SubmitEvent) {
    event.preventDefault()
    if (!input.value || sending) return
    const password = input.value
    input.value = ''
    sending = true
    error = ''
    try {
      await invoke('ttp_authenticate', { id: prompt.id, password })
    } catch (failure) {
      error = String(failure)
      sending = false
      input.focus()
    }
  }

  async function cancel(event: Event) {
    event.preventDefault()
    if (sending) return
    input.value = ''
    try {
      await invoke('ttp_cancel_authentication', { id: prompt.id })
    } catch (failure) {
      error = String(failure)
    }
  }
</script>

<dialog bind:this={dialog} class="auth-panel" aria-labelledby="auth-title" aria-describedby="auth-instruction" oncancel={cancel}>
  <form onsubmit={authenticate} autocomplete="off">
    <div class="flex items-center justify-between">
      <h2 id="auth-title" class="auth-title">Authentication</h2>
      <button type="button" class="icon-button close-glyph" aria-label="Cancel authentication" disabled={sending} onclick={cancel}>×</button>
    </div>
    <p id="auth-instruction">{operation === 'toggle' ? 'Enter your sudo password to read the session. TTP will start an inactive proxy or stop an active one.' : 'Enter your sudo password to ' + operations[operation] + '.'}</p>
    <label class="section-label" for="sudo-password">Password</label>
    <input bind:this={input} id="sudo-password" type="password" required maxlength="1024" spellcheck={false} autocomplete="off" />
    {#if prompt.rejected || error}<p class="auth-error" role="alert">{error || 'Password was rejected. Try again.'}</p>{/if}
    <button type="submit" class="primary-button auth-submit" disabled={sending}>Authenticate</button>
  </form>
</dialog>

<style>
  .auth-panel {
    inset: 0;
    width: min(320px, calc(100% - 42px));
    max-height: calc(100% - 42px);
    margin: auto;
    padding: 20px;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--button-border);
    border-radius: 16px;
    animation: reveal 250ms var(--spring) both;
  }

  .auth-panel::backdrop { background: rgb(0 0 0 / 76%); }
  .close-glyph { font-size: 20px; line-height: 1; }
  .auth-title { font-size: 15px; font-weight: 600; }
  #auth-instruction { margin: 6px 0 18px; color: var(--muted); font-size: 12px; line-height: 1.45; }
  input {
    display: block;
    width: 100%;
    height: 41px;
    margin: 7px 0 12px;
    padding: 0 11px;
    border: 1px solid var(--button-border);
    border-radius: 9px;
    background: var(--background);
    user-select: text;
  }

  .auth-error { margin: 0 0 12px; color: var(--muted); font-size: 11px; }
  .auth-submit { height: 42px; font-size: 13px; }
</style>
