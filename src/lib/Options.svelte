<script lang="ts">
  import { onMount } from 'svelte'
  import Icon from './Icon.svelte'
  import type { Operation, TtpOptions } from './ttp'

  let {
    active,
    retained,
    unknown,
    busy,
    desktop,
    watchdogRunning,
    options,
    ondone,
    onaction,
    onsaveoptions,
    onhide,
    onquit,
  }: {
    active: boolean
    retained: boolean
    unknown: boolean
    busy: boolean
    desktop: boolean
    watchdogRunning: boolean
    options: TtpOptions
    ondone: () => void
    onaction: (action: Operation) => void
    onsaveoptions: (next: TtpOptions) => void
    onhide: () => void
    onquit: () => void
  } = $props()

  let dialog: HTMLDialogElement
  let draft = $state<TtpOptions>({ ...options })

  onMount(() => {
    draft = { ...options }
    dialog.showModal()
    return () => dialog.close()
  })

  function toggleVerbose(checked: boolean) {
    draft.verbose = checked
    if (checked) draft.quiet = false
    onsaveoptions({ ...draft })
  }

  function toggleQuiet(checked: boolean) {
    draft.quiet = checked
    if (checked) draft.verbose = false
    onsaveoptions({ ...draft })
  }

  function commit() {
    onsaveoptions({
      ...draft,
      requireVpn: draft.requireVpn.trim(),
      extraStartFlags: draft.extraStartFlags.trim(),
    })
  }
</script>

<dialog class="options" bind:this={dialog} aria-labelledby="options-title" oncancel={(event) => { event.preventDefault(); ondone() }}>
  <header class="sheet-toolbar" data-tauri-drag-region>
    <h2 id="options-title" data-tauri-drag-region>Settings</h2>
    <button class="done-button" onclick={ondone}>Done</button>
  </header>
  <div class="sheet-body">
    <h3 class="section-label">Proxy session</h3>
    <div class="option-group">
      <button disabled={busy || !desktop} onclick={() => onaction(active || retained ? 'stop' : 'start')}>
        <Icon name="route"/><span>{retained ? 'Restore network' : active ? 'Stop Proxy' : 'Start Proxy'}</span>
      </button>
      {#if unknown}
        <button disabled={busy || !desktop} onclick={() => onaction('stop')}><Icon name="route"/><span>Stop Proxy</span></button>
      {/if}
      <button disabled={busy || !active || !desktop} onclick={() => onaction('restart')}><Icon name="rotate"/><span>Restart session</span></button>
      <button disabled={busy || !active || !desktop} onclick={() => onaction('refresh')}><Icon name="rotate"/><span>Rotate exit IP</span></button>
      <button disabled={busy || !active || !desktop} onclick={() => onaction('check')}><Icon name="check"/><span>Check Tor routing</span></button>
      <button disabled={busy || !desktop} onclick={() => onaction('check_leak')}><Icon name="check"/><span>Check for leaks (check-leak)</span></button>
    </div>

    <h3 class="section-label">Watchdog &amp; diagnostics</h3>
    <div class="option-group">
      <button disabled={busy || !desktop} onclick={() => onaction(watchdogRunning ? 'watchdog_stop' : 'watchdog_start')}>
        <Icon name="route"/><span>{watchdogRunning ? 'Stop background watchdog' : 'Start background watchdog'}</span>
      </button>
      <button disabled={busy || !desktop} onclick={() => onaction('diagnose')}><Icon name="gear"/><span>Run system diagnostic</span></button>
      <button disabled={busy || !desktop} onclick={() => onaction('logs')}><Icon name="route"/><span>View recent TTP logs</span></button>
    </div>

    <h3 class="section-label">TTP CLI flags</h3>
    <div class="option-group flags-group">
      <label class="flag-row">
        <div class="flag-info">
          <span class="flag-name">--verbose (-v)</span>
          <span class="flag-desc">Show debug output</span>
        </div>
        <input type="checkbox" class="switch" checked={draft.verbose} onchange={(e) => toggleVerbose(e.currentTarget.checked)} />
      </label>
      <label class="flag-row">
        <div class="flag-info">
          <span class="flag-name">--quiet (-q)</span>
          <span class="flag-desc">Suppress all output except errors</span>
        </div>
        <input type="checkbox" class="switch" checked={draft.quiet} onchange={(e) => toggleQuiet(e.currentTarget.checked)} />
      </label>
      <div class="flag-row">
        <div class="flag-info">
          <span class="flag-name">--log-format</span>
          <span class="flag-desc">CLI log format</span>
        </div>
        <select class="flag-select" bind:value={draft.logFormat} onchange={commit}>
          <option value="text">text</option>
          <option value="json">json</option>
        </select>
      </div>
      <label class="flag-row">
        <div class="flag-info">
          <span class="flag-name">Live watchdog polling</span>
          <span class="flag-desc">Poll watchdog &amp; anomalies</span>
        </div>
        <input type="checkbox" class="switch" bind:checked={draft.liveWatchdog} onchange={commit} />
      </label>
      <div class="flag-input-row">
        <label class="flag-name" for="require-vpn-input">--require-vpn &lt;UUID&gt;</label>
        <input id="require-vpn-input" class="flag-input" type="text" placeholder="Full-tunnel NM WireGuard profile UUID" spellcheck="false" autocomplete="off" bind:value={draft.requireVpn} onchange={commit} />
      </div>
      <div class="flag-input-row">
        <label class="flag-name" for="extra-flags-input">Extra start flags</label>
        <input id="extra-flags-input" class="flag-input" type="text" placeholder="Additional flags passed to ttp start" spellcheck="false" autocomplete="off" bind:value={draft.extraStartFlags} onchange={commit} />
      </div>
    </div>

    <h3 class="section-label">Application</h3>
    <div class="option-group">
      <button disabled={!desktop} onclick={onhide}><Icon name="hide"/><span>Hide to tray</span></button>
      <button disabled={!desktop || busy} onclick={onquit}><Icon name="quit"/><span>Quit TTP Desktop</span></button>
    </div>
    {#if active || retained}<p class="menu-note">Quitting leaves the current TTP session in place.</p>{/if}
  </div>
</dialog>
