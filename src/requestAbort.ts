/**
 * Central dispatcher for client-aborted request notifications.
 *
 * Flow: Rust's `RequestStateGuard::drop` detects that a response was never sent
 * (client disconnected and hyper dropped the handler future) -> invokes the C
 * callback -> C++ `c_request_aborted_callback` -> Nitro's `AsyncJSCallback`
 * switches back to the JS thread -> `onRequestAborted`.
 *
 * This map routes native `requestId` notifications to the corresponding JS
 * `req`/`res` objects (`req.aborted`, `req.emit('aborted')`, `res.destroyed`)
 * and triggers an `AbortSignal` for downstream cancellation (e.g. `fetch`).
 *
 * `AbortController` is expected from the host (React Native provides it via
 * `Libraries/Core/setUpXHR.js`, while headless JS hosts may omit it). When
 * absent, `signal` is `undefined` and the rest of abort notification still works;
 * we intentionally avoid polyfilling a custom `AbortController`.
 */

import type { HttpServer as NitroHttpServer } from './HttpServer.nitro'

/** Abort tracking state for an in-flight request. */
export interface RequestAbortEntry {
    /** Aborted on client disconnect; null if the host lacks AbortController. */
    controller: AbortController | null
    /** `controller.signal` when supported by the host, otherwise undefined. */
    signal: AbortSignal | undefined
    /**
     * Extra abort callbacks registered by the Node compatibility layer (`http.ts`)
     * to update `req.aborted` / `res.destroyed` and emit events.
     */
    listeners: Set<() => void>
    /** Whether an abort notification has already been processed (idempotency guard). */
    aborted: boolean
}

/** Whether the JS host provides AbortController (probed once at load). */
const HAS_ABORT_CONTROLLER = typeof AbortController !== 'undefined'

/**
 * Map of in-flight requests.
 *
 * Entries are removed on whichever happens first:
 *  1. An abort notification arrives (native guard drops once per request);
 *  2. The handler settles (resolves or rejects).
 *
 * An entry only remains if a handler never settles and the client never disconnects.
 */
const pendingRequests = new Map<string, RequestAbortEntry>()

function createEntry(): RequestAbortEntry {
    if (!HAS_ABORT_CONTROLLER) {
        return { controller: null, signal: undefined, listeners: new Set(), aborted: false }
    }
    const controller = new AbortController()
    return { controller, signal: controller.signal, listeners: new Set(), aborted: false }
}

/**
 * Register an in-flight request. Repeated calls for the same `requestId`
 * return the same entry.
 */
export function trackRequest(requestId: string): RequestAbortEntry {
    const existing = pendingRequests.get(requestId)
    if (existing) return existing
    const entry = createEntry()
    pendingRequests.set(requestId, entry)
    return entry
}

/** Unregister a request (idempotent). */
export function untrackRequest(requestId: string): void {
    pendingRequests.delete(requestId)
}

/** Current number of tracked in-flight requests (for diagnostics and leak checks). */
export function pendingRequestCount(): number {
    return pendingRequests.size
}

/**
 * Native client-disconnect handler registered via `installRequestAbortedHandler`.
 *
 * Runs `listeners` before aborting `signal` so `req.aborted` and `res.destroyed`
 * are already `true` when `signal` abort listeners execute.
 */
export function onRequestAborted(requestId: string): void {
    const entry = pendingRequests.get(requestId)
    if (!entry) {
        // Request already finalized; ignore late race notifications.
        return
    }
    if (entry.aborted) return
    entry.aborted = true

    // Remove before notifying so entries are not retained if the handler never settles.
    pendingRequests.delete(requestId)

    for (const listener of Array.from(entry.listeners)) {
        try {
            listener()
        } catch (e) {
            console.error('[http-server] request-abort listener threw:', e)
        }
    }

    if (entry.controller) {
        try {
            entry.controller.abort()
        } catch (e) {
            console.error('[http-server] AbortController.abort() threw:', e)
        }
    }
}

/**
 * Whether the native callback has been installed.
 *
 * The native side (`cpp/HybridHttpServer.cpp` `g_abortHandler`) holds a single
 * global callback (last write wins), so installing once is sufficient across all
 * server classes.
 */
let installed = false

/**
 * Install the abort handler on the native module (idempotent).
 *
 * Intentionally called from `wrapHandler` / `Server.listen()` rather than at
 * module top-level so importing the package without starting a server does not
 * require native mocks in unit tests.
 */
export function installRequestAbortedHandler(module: NitroHttpServer): void {
    if (installed) return
    installed = true
    module.setRequestAbortedHandler(onRequestAborted)
}

/** Test-only: reset the installation flag and pending request map. */
export function __resetRequestAbortForTests(): void {
    installed = false
    pendingRequests.clear()
}
