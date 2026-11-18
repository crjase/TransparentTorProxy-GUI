export type Operation =
  | 'start'
  | 'stop'
  | 'restart'
  | 'refresh'
  | 'check'
  | 'check_leak'
  | 'watchdog_start'
  | 'watchdog_stop'
  | 'diagnose'
  | 'logs'
  | 'toggle'

export type Phase = 'active' | 'inactive' | 'protection_retained'
export type Verification = 'unknown' | 'confirmed' | 'negative' | 'inconclusive'
export type WatchdogState = 'idle' | 'healthy' | 'warning' | 'anomaly'

export interface TtpStatus {
  phase: Phase
  exitIp: string | null
  startedAt: number | null
  country: string | null
  verification: Verification
  issue: string | null
}

export interface CheckResult {
  verification: Verification
  exitIp: string | null
  latencyMs: number | null
  country: string | null
}

export interface WatchdogSnapshot {
  state: WatchdogState
  label: string
  detail: string
  daemonRunning: boolean
}

export interface TtpOptions {
  verbose: boolean
  quiet: boolean
  logFormat: 'text' | 'json'
  requireVpn: string
  extraStartFlags: string
  liveWatchdog: boolean
}

export interface AuthPrompt {
  id: number
  rejected: boolean
}

export interface Snapshot {
  revision: number
  status: TtpStatus | null
  statusNeedsAuth: boolean
  statusError: string | null
  torVersion: string | null
  check: CheckResult | null
  watchdog: WatchdogSnapshot
  options: TtpOptions
  operation: Operation | null
  slow: boolean
  auth: AuthPrompt | null
  notice: string | null
}

export const operations: Record<Operation, string> = {
  toggle: 'read the session and switch the proxy',
  start: 'start the proxy',
  stop: 'stop the proxy',
  restart: 'restart the proxy',
  refresh: 'rotate the exit IP',
  check: 'check Tor routing',
  check_leak: 'check for network leaks',
  watchdog_start: 'start the background watchdog',
  watchdog_stop: 'stop the background watchdog',
  diagnose: 'run a system diagnostic',
  logs: 'load recent TTP logs',
}

export function uptime(startedAt: number | null | undefined, now: number) {
  if (startedAt == null) return '—'
  const seconds = Math.max(0, Math.floor(now / 1000) - startedAt)
  return [Math.floor(seconds / 3600), Math.floor(seconds / 60) % 60, seconds % 60]
    .map((part) => String(part).padStart(2, '0'))
    .join(':')
}
