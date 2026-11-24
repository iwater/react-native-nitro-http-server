/**
 * Serialization gate for native server stop -> start transitions.
 *
 * `stop()` initiates native teardown asynchronously; starting a new server
 * before teardown finishes races with the old server, which can leave the new
 * server reporting a bound port while failing to serve requests.
 *
 * State is module-level rather than per-instance because the native server is a
 * singleton (C++ `g_serverContext` / `g_serverRunning` / `g_serverPort`), so
 * closing one JS server instance and immediately starting another also collides.
 */

let _pending: Promise<unknown> | null = null

/** Register an in-flight native stop promise without replacing the caller's promise. */
export function trackNativeStop(p: Promise<unknown>): void {
  _pending = p
  const clear = () => {
    if (_pending === p) _pending = null
  }
  // Clear on both fulfillment and rejection so a failed stop does not block future starts.
  p.then(clear, clear)
}

/**
 * Wait for any in-flight native stop to settle.
 * Proceeds even if the stop fails so a failed stop cannot permanently block start.
 */
export async function awaitPendingNativeStop(): Promise<void> {
  while (_pending) {
    const p = _pending
    try {
      await p
    } catch (_) {
      /* Do not block start if stop fails */
    }
    if (_pending === p) _pending = null
  }
}

/**
 * Track an in-flight native stop and await its completion.
 * Registers the stop for subsequent start calls while still propagating errors.
 */
export async function stopWithTracking(p: Promise<unknown>): Promise<void> {
  trackNativeStop(p)
  await p
}
