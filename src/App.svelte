<script lang="ts">
  import { onMount, tick } from 'svelte'
  import { invoke, isTauri } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import AuthDialog from './lib/AuthDialog.svelte'
  import Icon from './lib/Icon.svelte'
  import Options from './lib/Options.svelte'
  import { uptime } from './lib/ttp'
  import type { Operation, Snapshot, TtpOptions } from './lib/ttp'
  import mark from './assets/tp-mark.webp'
  import { version } from '../package.json'

  const progress: Record<Operation, string> = {
    toggle: 'Reading protected session',
    start: 'Starting proxy',
    stop: 'Stopping proxy',
    restart: 'Restarting proxy',
    refresh: 'Rotating exit IP',
    check: 'Checking Tor routing',
    check_leak: 'Checking for network leaks',
    watchdog_start: 'Starting watchdog',
    watchdog_stop: 'Stopping watchdog',
    diagnose: 'Running diagnostic',
    logs: 'Loading recent logs',
  }
  const phaseMessages = {
    inactive: 'No active TTP session.', active: 'Proxy session active.',
    protection_retained: 'TTP retained its network protection.',
  }
  type Entry = { id: number; time: string; text: string }
  const timestamp = new Intl.DateTimeFormat('en-GB', {
    hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false,
  })

  let view: Snapshot = $state({
    revision: -1,
    status: null,
    statusError: null,
    statusNeedsAuth: false,
    torVersion: null,
    check: null,
    watchdog: { state: 'idle', label: 'WATCHDOG · STANDBY', detail: 'No anomalies detected', daemonRunning: false },
    options: { verbose: false, quiet: false, logFormat: 'text', requireVpn: '', extraStartFlags: '', liveWatchdog: true },
    operation: null,
    slow: false,
    auth: null,
    notice: null,
  })
  let clock = $state(Date.now())
  let pending = $state<Operation | null>(null)
  let polling = $state(false)
  let desktop = $state(false)
  let clientError = $state('')
  let optionsOpen = $state(false)
  let menuButton: HTMLButtonElement
  let history: Entry[] = $state([])
  let nextId = 0
  let consoleView: HTMLDivElement

  let fresh = $derived(!view.statusError && !view.statusNeedsAuth)
  let phase = $derived(fresh ? view.status?.phase ?? 'unknown' : 'unknown')
  let active = $derived(phase === 'active')
  let retained = $derived(phase === 'protection_retained')
  let verified = $derived(active && !view.status?.issue && view.status?.verification === 'confirmed')
  let operation = $derived(view.operation ?? pending)
  let hasError = $derived(Boolean(view.statusError || clientError))
  let stages = $derived(phase === 'unknown' || retained ? null : Number(active) + Number(verified))
  let primary = $derived(phase === 'unknown' ? 'Start / Stop Proxy' : retained ? 'Restore network' : active ? 'Stop Proxy' : 'Start Proxy')
  let heading = $derived.by(() => {
    if (view.auth) return 'Authentication required'
    if (operation) return progress[operation] + (view.slow ? ' · still running' : '…')
    if (view.statusNeedsAuth) return 'Session requires authentication'
    if (phase === 'unknown') return hasError ? 'Session could not be read' : 'Reading session status…'
    if (retained) return 'Network restore required'
    if (verified) return 'Tor routing confirmed'
    if (active && (view.status?.issue || view.status?.verification === 'negative')) return 'Routing needs attention'
    return active ? 'Tor routing unconfirmed' : 'Ready to start proxy'
  })

  function log(text: string) {
    const follow = !consoleView || consoleView.scrollHeight - consoleView.scrollTop - consoleView.clientHeight < 10
    history = [...history.slice(-79), { id: ++nextId, time: timestamp.format(new Date()), text }]
    if (follow) void tick().then(() => consoleView?.scrollTo(0, consoleView.scrollHeight))
  }

  function report(failure: unknown) {
    const text = String(failure)
    if (clientError !== text && history.at(-1)?.text !== text) log(text)
    clientError = text
  }

  function apply(next: Snapshot) {
    if (next.revision <= view.revision) return
    const old = view
    view = next
    const status = next.status
    if (next.statusError && next.statusError !== old.statusError) log(next.statusError)
    if (next.statusNeedsAuth && !old.statusNeedsAuth) log('Session record is private. Use the blue button to authenticate.')
    if (status && !next.statusError && !next.statusNeedsAuth) {
      if (status.phase !== old.status?.phase || old.statusError || old.statusNeedsAuth) log(phaseMessages[status.phase])
      if (status.phase === 'active' && status.exitIp && status.exitIp !== old.status?.exitIp) log('Exit IP: ' + status.exitIp)
      if (status.issue && status.issue !== old.status?.issue) log(status.issue)
    }
    if (next.watchdog?.state === 'anomaly' && next.watchdog.detail !== old.watchdog?.detail) {
      log('Watchdog anomaly: ' + next.watchdog.detail)
    }
    if (next.operation && next.operation !== old.operation) log(progress[next.operation] + '…')
    if (next.slow && !old.slow) log('Waiting for the TTP command to finish.')
    if (next.check && old.operation === 'check' && !next.operation) {
      const label = next.check.verification === 'confirmed' ? 'Tor routing confirmed' :
        next.check.verification === 'negative' ? 'Tor check failed' : 'Tor routing unconfirmed'
      log(label + (next.check.latencyMs == null ? '.' : ' · ' + next.check.latencyMs + ' ms'))
    }
    if (next.notice && (next.notice !== old.notice || old.operation !== next.operation)) log(next.notice)
  }

  async function poll() {
    if (!desktop || polling || operation || document.visibilityState === 'hidden') return
    polling = true
    try {
      if (await getCurrentWindow().isVisible()) {
        const result = await invoke<Snapshot>('ttp_status')
        clientError = ''
        apply(result)
      }
    } catch (failure) {
      report(failure)
    } finally {
      polling = false
    }
  }

  async function closeOptions() {
    optionsOpen = false
    await tick()
    menuButton?.focus({ preventScroll: true })
  }

  async function run(action: Operation) {
    if (!desktop || operation) return
    if (optionsOpen) await closeOptions()
    pending = action
    clientError = ''
    try {
      await invoke('ttp_' + action)
    } catch (failure) {
      report(failure)
    } finally {
      pending = null
    }
  }

  async function saveOptions(next: TtpOptions) {
    if (!desktop) return
    try {
      const updated = await invoke<Snapshot>('ttp_save_options', { options: next })
      apply(updated)
    } catch (failure) {
      report(failure)
    }
  }

  async function hide() {
    if (optionsOpen) await closeOptions()
    try { await getCurrentWindow().close() } catch (failure) { report(failure) }
  }

  async function quit() {
    try { await invoke('quit_desktop') }
    catch (failure) { await closeOptions(); report(failure) }
  }

  onMount(() => {
    desktop = isTauri()
    if (!desktop) {
      report('Open TTP Desktop from your application launcher to connect to the CLI.')
      return
    }
    let mounted = true
    const cleanups: Array<() => void> = []
    async function subscribe() {
      try {
        const stopSnapshot = await listen<Snapshot>('ttp://snapshot', ({ payload }) => {
          if (payload.operation && !view.operation) clientError = ''
          apply(payload)
        })
        if (!mounted) { stopSnapshot(); return }
        cleanups.push(stopSnapshot)
        const stopFocus = await getCurrentWindow().onFocusChanged(({ payload }) => { if (payload) void poll() })
        if (!mounted) { stopFocus(); return }
        cleanups.push(stopFocus)
      } catch (failure) { report(failure) }
      if (mounted) void poll()
    }
    void subscribe()
    const updates = window.setInterval(poll, 5_000)
    const time = window.setInterval(() => { clock = Date.now() }, 1_000)
    document.addEventListener('visibilitychange', poll)
    return () => {
      mounted = false
      cleanups.forEach((stop) => stop())
      window.clearInterval(updates)
      window.clearInterval(time)
      document.removeEventListener('visibilitychange', poll)
    }
  })
</script>

<div class="frame" inert={optionsOpen || Boolean(view.auth)}>
  <header class="toolbar" data-tauri-drag-region>
    <button bind:this={menuButton} class="icon-button" aria-label="Open settings" title="Settings" aria-haspopup="dialog" aria-expanded={optionsOpen} onclick={() => { optionsOpen = true }}><Icon name="gear"/></button>
    <span class="window-title" data-tauri-drag-region>TTP Desktop</span>
    <button class="icon-button" aria-label="Hide to tray" title="Hide to tray" disabled={!desktop} onclick={hide}><Icon name="close"/></button>
  </header>

  <main>
    <div class="identity" data-tauri-drag-region>
      <img class="brand-mark" src={mark} alt="" draggable="false" data-tauri-drag-region/>
      <h1 data-tauri-drag-region><strong>TTP</strong> transparent proxy</h1>
      <p class="runtime-line" data-tauri-drag-region>for Arch Linux <span>·</span> x86_64</p>
      <p class="runtime-line" data-tauri-drag-region>{view.torVersion ? 'Tor ' + view.torVersion : 'Tor'} <span>·</span> nftables</p>
    </div>

    <section class="session" aria-label="Session readiness">
      <div class="fraction" aria-label={stages == null ? 'Session readiness unknown' : stages + ' of 2 readiness checks satisfied'} title="One stage for an active session, one for confirmed Tor routing">{stages ?? '—'}<span>/2</span></div>
      <p class="heading" role="status">{heading}</p>
      <div class="meter" aria-hidden="true"><i class:half={active} class:full={verified}></i></div>
      {#if active && view.status}
        <dl class="readouts">
          <div class="exit-row"><dt>Exit IP</dt><dd title={view.status.exitIp ?? undefined}>{#key view.status.exitIp}<span class="swap">{view.status.exitIp ?? '—'}</span>{/key}</dd></div>
          <div><dt>Uptime</dt><dd>{uptime(view.status.startedAt, clock)}</dd></div>
          {#if view.status.country}<div><dt>Country</dt><dd>{view.status.country}</dd></div>{/if}
        </dl>
      {/if}

      <div class="watchdog-strip" data-state={view.watchdog.state} title="Live TTP Watchdog & Anomaly Monitor">
        <div class="watchdog-left">
          <span class="watchdog-dot" aria-hidden="true"></span>
          <div class="watchdog-copy">
            <span class="watchdog-label">{view.watchdog.label}</span>
            <span class="watchdog-detail">{view.watchdog.detail}</span>
          </div>
        </div>
        <button type="button" class="watchdog-action" disabled={!desktop || operation !== null} onclick={() => run('check_leak')} title="Run ttp check-leak">Check</button>
      </div>
    </section>

    <section class="activity" aria-labelledby="log-title">
      <h2 class="sr-only" id="log-title">Session log</h2>
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard users can scroll the session log) -->
      <div bind:this={consoleView} class="log-scroll" tabindex="0" role="region" aria-label="Session log">
        {#each history as entry (entry.id)}
          <p class="log-line" title={entry.time}><span class="log-prefix" aria-hidden="true">[+]</span><span>{entry.text}</span></p>
        {/each}
      </div>
    </section>

    <footer>
      <button class="primary-button toggle" disabled={!desktop || operation !== null} aria-busy={Boolean(operation)} onclick={() => run(phase === 'unknown' ? 'toggle' : active || retained ? 'stop' : 'start')}>
        {#if operation === 'refresh'}<Icon name="rotate" spinning={true}/>{:else if operation}<span class="loading-dots" aria-hidden="true"><i></i><i></i><i></i></span>{/if}
        {operation ? progress[operation] : primary}
      </button>
      <p class="version">Version: {version}</p>
    </footer>
  </main>
</div>

{#if optionsOpen}
  <Options
    {active}
    {retained}
    unknown={phase === 'unknown'}
    busy={operation !== null}
    {desktop}
    watchdogRunning={view.watchdog.daemonRunning}
    options={view.options}
    ondone={closeOptions}
    onaction={run}
    onsaveoptions={saveOptions}
    onhide={hide}
    onquit={quit}
  />
{/if}

{#if view.auth && operation}
  {#key view.auth.id}<AuthDialog prompt={view.auth} {operation}/>{/key}
{/if}
