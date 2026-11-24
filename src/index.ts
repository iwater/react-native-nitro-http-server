import { NitroModules } from 'react-native-nitro-modules'
import { AppState, AppStateStatus } from 'react-native'
import type { HttpServer as NitroHttpServer, HttpRequest, HttpResponse as NitroHttpResponse, ServerConfig, CorsConfig } from './HttpServer.nitro'
import { createServer } from './http'
import { LoopRef } from './loopRef'
import { awaitPendingNativeStop, stopWithTracking } from './nativeStopQueue'
import { installRequestAbortedHandler, trackRequest, untrackRequest } from './requestAbort'

// Redefine HttpResponse for User (User sees unified body)
export interface HttpResponse extends Omit<NitroHttpResponse, 'body' | 'binaryBody'> {
  body?: string | ArrayBuffer
}

/**
 * Request object passed to user handlers: native `HttpRequest` augmented with `signal`.
 *
 * Note: `signal` is an extension of this module (Node's `http.IncomingMessage` does
 * not provide it), allowing direct cancellation forwarding to `fetch(url, { signal })`
 * or downstream SDKs. When the host lacks `AbortController` (such as headless JS
 * runners), `signal` is `undefined`.
 */
export interface HttpRequestWithSignal extends HttpRequest {
  /** Aborted when the client disconnects before a response is sent. */
  readonly signal?: AbortSignal
}

// Redefine RequestHandler to use local HttpResponse
export type RequestHandler = (request: HttpRequestWithSignal) => Promise<HttpResponse> | HttpResponse

export interface ServerOptions {
  /** IP address to listen on (default: 127.0.0.1). */
  host?: string
  /**
   * Tokio worker thread count. Omit or 0 to use CPU core count (tokio default).
   *
   * The runtime is created on `start()` and destroyed on `stop()`, so this can be
   * changed between starts. Fewer threads reduce concurrency when handlers perform
   * slow `await`s (e.g. DB or upstream network), whereas reducing worker count on
   * high-core devices can lower mutex contention and per-request CPU overhead.
   */
  workerThreads?: number
  /**
   * Automatically detect and restart the server when returning to the foreground
   * after OS suspension. Defaults to false.
   */
  autoRestart?: boolean
}

// Normalize legacy string host argument or ServerOptions object.
function normalizeOptions(hostOrOptions?: string | ServerOptions): ServerOptions {
  if (typeof hostOrOptions === 'string') {
    return { host: hostOrOptions }
  }
  return hostOrOptions || {}
}

const HttpServerModule = NitroModules.createHybridObject<NitroHttpServer>("HttpServer")

// Helper function to wrap handler and intercept binary body
const wrapHandler = (handler: RequestHandler): (request: HttpRequest) => Promise<NitroHttpResponse> => {
  // Install the native abort handler lazily on server start rather than at module import.
  installRequestAbortedHandler(HttpServerModule)

  return async (request: HttpRequest) => {
    // Track abort state before invoking the handler so disconnects during awaits are captured.
    const abortEntry = trackRequest(request.requestId)
    const mutableRequest = request as HttpRequest & { signal?: AbortSignal }
    mutableRequest.signal = abortEntry.signal

    try {
      const response = await handler(mutableRequest)

      // If response body is binary (ArrayBuffer or View), send it safely via the direct API
      if (response.body && typeof response.body === 'object' &&
        (response.body instanceof ArrayBuffer || ArrayBuffer.isView(response.body))) {

        const binaryBody = response.body as ArrayBuffer | ArrayBufferView;
        const buffer = binaryBody instanceof ArrayBuffer
          ? binaryBody
          : (binaryBody.byteLength === binaryBody.buffer.byteLength && binaryBody.byteOffset === 0)
            ? binaryBody.buffer
            : binaryBody.buffer.slice(binaryBody.byteOffset, binaryBody.byteOffset + binaryBody.byteLength);

        const headers = response.headers || {}
        const headersJson = JSON.stringify(headers)
        // Use the safe native method that copies data on JS thread
        await HttpServerModule.sendBinaryResponse(
          request.requestId,
          response.statusCode,
          headersJson,
          buffer as ArrayBuffer
        )
        // Return a dummy response to satisfy the native promise
        return {
          statusCode: response.statusCode,
          headers: response.headers,
          body: '' // Body handled via sendBinaryResponse
        }
      }

      // String body or empty
      return response as NitroHttpResponse
    } finally {
      untrackRequest(request.requestId)
    }
  }
}

/**
 * Per-request loop ref: acquired before an asynchronous native call and released on settle.
 *
 * Complements handle-level refs (which cover a running server from `start()` to `stop()`):
 * one-shot async methods have no server handle keeping a headless event loop alive, so
 * without a request ref the loop could exit before the native result is delivered.
 */
async function withRequestRef<T>(label: string, run: () => Promise<T>): Promise<T> {
  const ref = new LoopRef(label)
  ref.acquire()
  try {
    return await run()
  } finally {
    ref.release()
  }
}

export class HttpServer {
  /**
   * Keeps the event loop alive while the server is running or starting.
   * Native `start()` is asynchronous, so without this ref a headless loop could
   * exit before the start promise resolves.
   */
  private _loopRef = new LoopRef('http-server.HttpServer')
  private _isRunning = false
  private _port = 0
  private _handler: RequestHandler | null = null
  private _options: ServerOptions = {}
  private _appStateSub: { remove: () => void } | null = null
  private _intentionallyStopped = false

  async start(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<number> {
    // Wait for any pending native stop to finish (singleton native server; see nativeStopQueue.ts).
    await awaitPendingNativeStop()
    if (this._isRunning) {
      throw new Error('Server is already running')
    }

    this._options = normalizeOptions(hostOrOptions)
    this._handler = handler
    this._intentionallyStopped = false

    const wrappedHandler = wrapHandler(handler)
    const host = this._options.host
    // Keep the event loop alive during async native start.
    this._loopRef.acquire()
    let actualPort = 0
    try {
      actualPort = await HttpServerModule.start(port, wrappedHandler, host, this._options.workerThreads)
    } catch (e) {
      this._loopRef.release()
      throw e
    }
    if (actualPort <= 0) this._loopRef.release()
    this._isRunning = actualPort > 0
    this._port = actualPort

    if (this._options.autoRestart) {
      this._registerAutoRestart()
    }

    return actualPort
  }

  get port(): number {
    return this._port
  }

  async stop(): Promise<void> {
    this._intentionallyStopped = true
    this._unregisterAutoRestart()

    if (!this._isRunning) return

    await stopWithTracking(HttpServerModule.stop())
    this._loopRef.release()
    this._isRunning = false
  }

  async getStats(): Promise<Record<string, any>> {
    return await withRequestRef('http-server.getStats', () => HttpServerModule.getStats())
  }

  async isRunning(): Promise<boolean> {
    // Native isRunning() probes the socket via TCP connect to verify it is still alive.
    const nativeRunning = await withRequestRef('http-server.isRunning', () => HttpServerModule.isRunning())
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // Probe this instance's own port independently via HEAD request.
    // Use a 500ms timeout and retry once after 100ms: the JS thread may be busy when
    // returning to the foreground, and too short a timeout would falsely restart a healthy server.
    const probe = async (): Promise<boolean> => {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 500);
      try {
        await fetch(`http://127.0.0.1:${this._port}/`, {
          method: 'HEAD',
          signal: controller.signal,
        });
        return true;
      } catch {
        return false;
      } finally {
        // Clear the timer so immediate connection-refused rejections do not leave dangling timers.
        clearTimeout(timeout);
      }
    };
    if (await probe()) return true;
    await new Promise(r => setTimeout(r, 100));
    return probe();
  }

  private _registerAutoRestart(): void {
    if (this._appStateSub) return

    this._appStateSub = AppState.addEventListener('change', async (state: AppStateStatus) => {
      if (state !== 'active') return
      if (this._intentionallyStopped) return

      const alive = await this._probeAlive()
      if (!alive && this._handler) {
        console.log('[HttpServer] Detected server is dead, auto-restarting...')
        this._isRunning = false
        try {
          // Clean up any leftover native state.
          await stopWithTracking(HttpServerModule.stop())
          this._loopRef.release()
        } catch (_) { /* ignore */ }

        try {
          const wrappedHandler = wrapHandler(this._handler)
          const host = this._options.host
          this._loopRef.acquire()
          const actualPort = await HttpServerModule.start(this._port, wrappedHandler, host, this._options.workerThreads)
          if (actualPort > 0) {
            this._isRunning = true
            this._port = actualPort
            console.log(`[HttpServer] Auto-restarted on port ${actualPort}`)
          } else {
            this._loopRef.release()
          }
        } catch (e) {
          this._loopRef.release()
          console.error('[HttpServer] Auto-restart failed:', e)
        }
      }
    })
  }

  private _unregisterAutoRestart(): void {
    if (this._appStateSub) {
      this._appStateSub.remove()
      this._appStateSub = null
    }
  }
}

export class StaticServer {
  /** Same as HttpServer: keeps the event loop alive while running; released on stop(). */
  private _loopRef = new LoopRef('http-server.StaticServer')
  private _isRunning = false
  private _port = 0
  private _rootDir = ''
  private _options: ServerOptions = {}
  private _appStateSub: { remove: () => void } | null = null
  private _intentionallyStopped = false

  async start(port: number, rootDir: string, hostOrOptions?: string | ServerOptions): Promise<number> {
    await awaitPendingNativeStop()
    if (this._isRunning) {
      throw new Error('Static server is already running')
    }

    this._options = normalizeOptions(hostOrOptions)
    this._rootDir = rootDir
    this._intentionallyStopped = false

    const host = this._options.host
    this._loopRef.acquire()
    let actualPort = 0
    try {
      actualPort = await HttpServerModule.startStaticServer(port, rootDir, host)
    } catch (e) {
      this._loopRef.release()
      throw e
    }
    if (actualPort <= 0) this._loopRef.release()
    this._isRunning = actualPort > 0
    this._port = actualPort

    if (this._options.autoRestart) {
      this._registerAutoRestart()
    }

    return actualPort
  }

  get port(): number {
    return this._port
  }

  async stop(): Promise<void> {
    this._intentionallyStopped = true
    this._unregisterAutoRestart()

    if (!this._isRunning) return

    await stopWithTracking(HttpServerModule.stopStaticServer())
    this._loopRef.release()
    this._isRunning = false
  }

  async isRunning(): Promise<boolean> {
    const nativeRunning = await withRequestRef('http-server.isRunning', () => HttpServerModule.isRunning())
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // Use a 500ms timeout and retry once after 100ms: the JS thread may be busy when
    // returning to the foreground, and too short a timeout would falsely restart a healthy server.
    const probe = async (): Promise<boolean> => {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 500);
      try {
        await fetch(`http://127.0.0.1:${this._port}/`, {
          method: 'HEAD',
          signal: controller.signal,
        });
        return true;
      } catch {
        return false;
      } finally {
        // Clear the timer so immediate connection-refused rejections do not leave dangling timers.
        clearTimeout(timeout);
      }
    };
    if (await probe()) return true;
    await new Promise(r => setTimeout(r, 100));
    return probe();
  }

  private _registerAutoRestart(): void {
    if (this._appStateSub) return

    this._appStateSub = AppState.addEventListener('change', async (state: AppStateStatus) => {
      if (state !== 'active') return
      if (this._intentionallyStopped) return

      const alive = await this._probeAlive()
      if (!alive) {
        console.log('[StaticServer] Detected server is dead, auto-restarting...')
        this._isRunning = false
        // Bypasses stop(), so release the loop ref manually if native stop succeeds.
        try { await stopWithTracking(HttpServerModule.stopStaticServer()); this._loopRef.release() } catch (_) { /* ignore */ }

        try {
          const host = this._options.host
          this._loopRef.acquire()
          const actualPort = await HttpServerModule.startStaticServer(this._port, this._rootDir, host)
          if (actualPort > 0) {
            this._isRunning = true
            this._port = actualPort
            console.log(`[StaticServer] Auto-restarted on port ${actualPort}`)
          } else {
            this._loopRef.release()
          }
        } catch (e) {
          this._loopRef.release()
          console.error('[StaticServer] Auto-restart failed:', e)
        }
      }
    })
  }

  private _unregisterAutoRestart(): void {
    if (this._appStateSub) {
      this._appStateSub.remove()
      this._appStateSub = null
    }
  }
}

export class AppServer {
  /** Same as HttpServer: keeps the event loop alive while running; released on stop(). */
  private _loopRef = new LoopRef('http-server.AppServer')
  private _isRunning = false
  private _port = 0
  private _handler: RequestHandler | null = null
  private _rootDir = ''
  private _options: ServerOptions = {}
  private _appStateSub: { remove: () => void } | null = null
  private _intentionallyStopped = false

  async start(port: number, rootDir: string, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<number> {
    await awaitPendingNativeStop()
    if (this._isRunning) {
      throw new Error('App server is already running')
    }

    this._options = normalizeOptions(hostOrOptions)
    this._handler = handler
    this._rootDir = rootDir
    this._intentionallyStopped = false

    const wrappedHandler = wrapHandler(handler)
    const host = this._options.host
    this._loopRef.acquire()
    let actualPort = 0
    try {
      actualPort = await HttpServerModule.startAppServer(port, rootDir, wrappedHandler, host)
    } catch (e) {
      this._loopRef.release()
      throw e
    }
    if (actualPort <= 0) this._loopRef.release()
    this._isRunning = actualPort > 0
    this._port = actualPort

    if (this._options.autoRestart) {
      this._registerAutoRestart()
    }

    return actualPort
  }

  get port(): number {
    return this._port
  }

  async stop(): Promise<void> {
    this._intentionallyStopped = true
    this._unregisterAutoRestart()

    if (!this._isRunning) return

    await stopWithTracking(HttpServerModule.stopAppServer())
    this._loopRef.release()
    this._isRunning = false
  }

  async isRunning(): Promise<boolean> {
    const nativeRunning = await withRequestRef('http-server.isRunning', () => HttpServerModule.isRunning())
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // Use a 500ms timeout and retry once after 100ms: the JS thread may be busy when
    // returning to the foreground, and too short a timeout would falsely restart a healthy server.
    const probe = async (): Promise<boolean> => {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 500);
      try {
        await fetch(`http://127.0.0.1:${this._port}/`, {
          method: 'HEAD',
          signal: controller.signal,
        });
        return true;
      } catch {
        return false;
      } finally {
        // Clear the timer so immediate connection-refused rejections do not leave dangling timers.
        clearTimeout(timeout);
      }
    };
    if (await probe()) return true;
    await new Promise(r => setTimeout(r, 100));
    return probe();
  }

  private _registerAutoRestart(): void {
    if (this._appStateSub) return

    this._appStateSub = AppState.addEventListener('change', async (state: AppStateStatus) => {
      if (state !== 'active') return
      if (this._intentionallyStopped) return

      const alive = await this._probeAlive()
      if (!alive && this._handler) {
        console.log('[AppServer] Detected server is dead, auto-restarting...')
        this._isRunning = false
        // Bypasses stop(), so release the loop ref manually if native stop succeeds.
        try { await stopWithTracking(HttpServerModule.stopAppServer()); this._loopRef.release() } catch (_) { /* ignore */ }

        try {
          const wrappedHandler = wrapHandler(this._handler)
          const host = this._options.host
          this._loopRef.acquire()
          const actualPort = await HttpServerModule.startAppServer(this._port, this._rootDir, wrappedHandler, host)
          if (actualPort > 0) {
            this._isRunning = true
            this._port = actualPort
            console.log(`[AppServer] Auto-restarted on port ${actualPort}`)
          } else {
            this._loopRef.release()
          }
        } catch (e) {
          this._loopRef.release()
          console.error('[AppServer] Auto-restart failed:', e)
        }
      }
    })
  }

  private _unregisterAutoRestart(): void {
    if (this._appStateSub) {
      this._appStateSub.remove()
      this._appStateSub = null
    }
  }
}

/** Incoming WebSocket connection handshake metadata. */
export interface WebSocketConnectionRequest {
  path: string
  query: string
  headers: Record<string, string>
}

export type WebSocketConnectionHandler = (ws: ServerWebSocket, request: WebSocketConnectionRequest) => void

/** Configurable HTTP server supporting WebDAV, Zip mounts, WebSocket, and other plugins. */
export class ConfigServer {
  /** Same as HttpServer: keeps the event loop alive while running; released on stop(). */
  private _loopRef = new LoopRef('http-server.ConfigServer')
  private _isRunning = false
  private _port = 0
  private _wsEnabled = false
  private _wsHandlers: Map<string, WebSocketConnectionHandler> = new Map()
  private _handler: RequestHandler | null = null
  private _config: ServerConfig | null = null
  private _options: ServerOptions = {}
  private _appStateSub: { remove: () => void } | null = null
  private _intentionallyStopped = false

  get port(): number {
    return this._port
  }

  /**
   * Register a WebSocket connection handler.
   * @param path WebSocket endpoint path (e.g. '/ws')
   * @param handler Connection handler
   */
  onWebSocket(path: string, handler: WebSocketConnectionHandler): this {
    this._wsHandlers.set(path, handler)
    return this
  }

  async start(port: number, handler: RequestHandler, config: ServerConfig, hostOrOptions?: string | ServerOptions): Promise<number> {
    await awaitPendingNativeStop()
    if (this._isRunning) {
      throw new Error('Config server is already running')
    }

    this._options = normalizeOptions(hostOrOptions)
    this._handler = handler
    this._config = config
    this._intentionallyStopped = false

    if (config.mounts) {
      const wsMount = config.mounts.find((m: any) => m.type === 'websocket')
      if (wsMount) {
        this._wsEnabled = true
      }
    }

    const wrappedHandler = wrapHandler(handler)
    const configJson = JSON.stringify(config)
    // Apply CORS config (explicitly disabled when omitted to avoid stale state from previous starts).
    // Not repeated on auto-restart because Rust CORS_CONFIG is global state preserved across stopAppServer.
    HttpServerModule.setCorsConfig(JSON.stringify(config.cors ?? false))
    const host = this._options.host
    this._loopRef.acquire()
    let actualPort = 0
    try {
      actualPort = await HttpServerModule.startServerWithConfig(port, wrappedHandler, configJson, host)
    } catch (e) {
      this._loopRef.release()
      throw e
    }
    if (actualPort <= 0) this._loopRef.release()
    this._isRunning = actualPort > 0
    this._port = actualPort

    if (actualPort > 0 && this._wsEnabled) {
      this._setupWebSocketHandler()
    }

    if (this._options.autoRestart) {
      this._registerAutoRestart()
    }

    return actualPort
  }

  private _setupWebSocketHandler(): void {
    // Capture handlers in closure for the native callback.
    const wsHandlers = this._wsHandlers

    const eventHandler = (event: import('./HttpServer.nitro').WebSocketEvent) => {
      let ws = webSocketConnections.get(event.connectionId)

      if (event.type === 'open') {
        ws = new ServerWebSocket(event.connectionId)
        webSocketConnections.set(event.connectionId, ws)

        let headers: Record<string, string> = {}
        if (event.headersJson) {
          try {
            headers = JSON.parse(event.headersJson)
          } catch (e) {
            console.error('[WebSocket] Failed to parse headers JSON:', e)
          }
        }

        const request: WebSocketConnectionRequest = {
          path: event.path || '/ws',
          query: event.query || '',
          headers,
        }

        const handlerPath = event.path || '/ws'
        const handler = wsHandlers.get(handlerPath) || wsHandlers.get('*')

        if (handler) {
          try {
            handler(ws, request)
          } catch (e) {
            console.error('[WebSocket] Handler error on open:', e)
          }
        } else {
          console.warn(`[WebSocket] No handler registered for path: ${handlerPath}`)
        }

        ws._handleOpen()

      } else if (ws) {
        switch (event.type) {
          case 'message':
            const data = event.binaryData || event.textData || ''
            ws._handleMessage(data)
            break
          case 'close':
            ws._handleClose(event.closeCode || 1000, event.closeReason || '')
            webSocketConnections.delete(event.connectionId)
            break
          case 'error':
            ws._handleError(event.errorMessage || 'Unknown error')
            break
        }
      }
    }

    HttpServerModule.setWebSocketHandler(eventHandler)
    // Remember the last native handler installed so autoRestart restores the same owner.
    reinstallWsHandler = eventHandler
  }

  /**
   * Close and remove all entries from the module-level WebSocket connection map.
   *
   * Because `webSocketConnections` is module-level, dead connections would otherwise
   * leak and remain visible in `getWebSocketConnections()` across restarts.
   * Once an entry is deleted, native close events can no longer reach the instance,
   * so `_handleClose` is invoked immediately upon deletion (idempotent with native close).
   */
  private async _closeAllWebSocketConnections(): Promise<void> {
    for (const [id, ws] of webSocketConnections) {
      try {
        await ws.close(1001, 'server stopped')
      } catch (_) { /* ignore */ }
      // If delete() returns false, the native close event already removed and closed it.
      if (webSocketConnections.delete(id)) {
        ws._handleClose(1001, 'server stopped')
      }
    }
  }

  async stop(): Promise<void> {
    this._intentionallyStopped = true
    this._unregisterAutoRestart()

    // Close tracked WebSocket connections before early-returning on !_isRunning,
    // because isRunning() may have updated _isRunning to false after a native crash/suspend
    // while stale JS connection entries are still in the map.
    await this._closeAllWebSocketConnections()

    if (!this._isRunning) return

    await stopWithTracking(HttpServerModule.stopAppServer())
    this._loopRef.release()
    this._isRunning = false
    this._wsEnabled = false
    this._wsHandlers.clear()
  }

  async isRunning(): Promise<boolean> {
    const nativeRunning = await withRequestRef('http-server.isRunning', () => HttpServerModule.isRunning())
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // Use a 500ms timeout and retry once after 100ms: the JS thread may be busy when
    // returning to the foreground, and too short a timeout would falsely restart a healthy server.
    const probe = async (): Promise<boolean> => {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 500);
      try {
        await fetch(`http://127.0.0.1:${this._port}/`, {
          method: 'HEAD',
          signal: controller.signal,
        });
        return true;
      } catch {
        return false;
      } finally {
        // Clear the timer so immediate connection-refused rejections do not leave dangling timers.
        clearTimeout(timeout);
      }
    };
    if (await probe()) return true;
    await new Promise(r => setTimeout(r, 100));
    return probe();
  }

  private _registerAutoRestart(): void {
    if (this._appStateSub) return

    this._appStateSub = AppState.addEventListener('change', async (state: AppStateStatus) => {
      if (state !== 'active') return
      if (this._intentionallyStopped) return

      const alive = await this._probeAlive()
      if (!alive && this._handler && this._config) {
        console.log('[ConfigServer] Detected server is dead, auto-restarting...')
        this._isRunning = false
        try {
          await stopWithTracking(HttpServerModule.stopAppServer())
          this._loopRef.release()
        } catch (_) { /* ignore */ }

        try {
          const wrappedHandler = wrapHandler(this._handler)
          const configJson = JSON.stringify(this._config)
          const host = this._options.host
          this._loopRef.acquire()
          const actualPort = await HttpServerModule.startServerWithConfig(this._port, wrappedHandler, configJson, host)
          if (actualPort > 0) {
            this._isRunning = true
            this._port = actualPort
            // Previous server stopped: purge stale connections from the module-level map.
            await this._closeAllWebSocketConnections()
            // Re-install the active WebSocket handler by its pre-restart owner (global
            // single native callback; avoids overwriting a standalone setupWebSocketHandler).
            if (reinstallWsHandler) {
              HttpServerModule.setWebSocketHandler(reinstallWsHandler)
            } else if (this._wsEnabled) {
              this._setupWebSocketHandler()
            }
            console.log(`[ConfigServer] Auto-restarted on port ${actualPort}`)
          } else {
            this._loopRef.release()
          }
        } catch (e) {
          this._loopRef.release()
          console.error('[ConfigServer] Auto-restart failed:', e)
        }
      }
    })
  }

  private _unregisterAutoRestart(): void {
    if (this._appStateSub) {
      this._appStateSub.remove()
      this._appStateSub = null
    }
  }
}

/**
 * Create and start an HTTP server.
 * @param port Port number to bind.
 * @param handler Request handler callback.
 * @param hostOrOptions Host string or server options (`host`, `workerThreads`, `autoRestart`).
 */
export async function createHttpServer(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<HttpServer> {
  const server = new HttpServer()
  await server.start(port, handler, hostOrOptions)
  return server
}

/**
 * Create and start a static file server.
 * @param port Port number to bind.
 * @param rootDir Static file root directory path.
 * @param hostOrOptions Host string or server options (`host`, `autoRestart`).
 */
export async function createStaticServer(port: number, rootDir: string, hostOrOptions?: string | ServerOptions): Promise<StaticServer> {
  const server = new StaticServer()
  await server.start(port, rootDir, hostOrOptions)
  return server
}

/**
 * Create and start an App HTTP server (hybrid static files and callback handler).
 * @param port Port number to bind.
 * @param rootDir Static file root directory path.
 * @param handler Request handler callback.
 * @param hostOrOptions Host string or server options (`host`, `autoRestart`).
 */
export async function createAppServer(port: number, rootDir: string, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<AppServer> {
  const server = new AppServer()
  await server.start(port, rootDir, handler, hostOrOptions)
  return server
}

/**
 * Create and start a configurable App HTTP server.
 * @param port Port number to bind.
 * @param handler Request handler callback.
 * @param config Plugin configuration (may include `root_dir` for static files).
 * @param hostOrOptions Host string or server options (`host`, `autoRestart`).
 */
export async function createConfigServer(port: number, handler: RequestHandler, config: ServerConfig, hostOrOptions?: string | ServerOptions): Promise<ConfigServer> {
  const server = new ConfigServer()
  await server.start(port, handler, config, hostOrOptions)
  return server
}

/**
 * Set global CORS configuration (applies to all server types; call before starting).
 * Invalid JSON does not throw; the Rust layer logs a warning and disables CORS.
 * @param config `true` for defaults (Allow-Origin: *), a custom `CorsConfig`, or `false` to disable.
 */
export function setCorsConfig(config: boolean | CorsConfig): void {
  HttpServerModule.setCorsConfig(JSON.stringify(config ?? false))
}

export type { HttpRequest, ServerConfig, CorsConfig, DirListConfig, Mountable, WebDavMount, ZipMount, StaticMount, UploadMount, BufferUploadMount, RewriteMount, RewriteRule, WebSocketMount, WebSocketEvent, WebSocketEventType, WebSocketHandler } from './HttpServer.nitro'

export { HttpServerModule }

/** Current count of tracked in-flight requests (for diagnostics; normally returns to 0). */
export { pendingRequestCount } from './requestAbort'

// ==================== WebSocket API ====================

/**
 * W3C WebSocket-compatible server-side WebSocket interface.
 */
export class ServerWebSocket {
  private _connectionId: string
  private _readyState: number = 0 // 0=CONNECTING, 1=OPEN, 2=CLOSING, 3=CLOSED

  public onopen: ((this: ServerWebSocket) => void) | null = null
  public onmessage: ((this: ServerWebSocket, event: { data: string | ArrayBuffer }) => void) | null = null
  public onclose: ((this: ServerWebSocket, event: { code: number; reason: string; wasClean: boolean }) => void) | null = null
  public onerror: ((this: ServerWebSocket, event: { message: string }) => void) | null = null

  constructor(connectionId: string) {
    this._connectionId = connectionId
    this._readyState = 1 // OPEN
  }

  get connectionId(): string {
    return this._connectionId
  }

  get readyState(): number {
    return this._readyState
  }

  /**
   * Send a text or binary message.
   *
   * Binary data accepts an `ArrayBuffer` or any `ArrayBufferView` (`TypedArray`, `DataView`,
   * or `Buffer`), normalized to an `ArrayBuffer` for native `wsSendBinary`.
   */
  async send(data: string | ArrayBuffer | ArrayBufferView): Promise<boolean> {
    if (this._readyState !== 1) {
      throw new Error('WebSocket is not open')
    }

    if (typeof data === 'string') {
      return await HttpServerModule.wsSendText(this._connectionId, data)
    }

    // Normalize view to ArrayBuffer, reusing `.buffer` only when the view spans the full buffer
    // to avoid leaking surrounding bytes from subarrays or pooled Buffers.
    const buffer = data instanceof ArrayBuffer
      ? data
      : (data.byteLength === data.buffer.byteLength && data.byteOffset === 0)
        ? data.buffer
        : data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength)
    return await HttpServerModule.wsSendBinary(this._connectionId, buffer as ArrayBuffer)
  }

  /**
   * Close the WebSocket connection.
   *
   * @returns `true` if the close frame was queued (executes after previously queued messages);
   *          `false` if not queued (unknown connection, full send queue, or already closed),
   *          in which case an open connection may still be retried.
   */
  async close(code: number = 1000, reason: string = ''): Promise<boolean> {
    if (this._readyState >= 2) {
      return false
    }

    this._readyState = 2 // CLOSING
    const result = await HttpServerModule.wsClose(this._connectionId, code, reason)

    if (result) {
      // Queued: send_task will close after flushing preceding messages.
      // Mark CLOSED logically here; native close will idempotently set it again via _handleClose.
      this._readyState = 3 // CLOSED
    } else {
      // Not queued: revert to OPEN so the caller can retry (subsequent send/close will
      // return false if already disconnected, and native close still triggers _handleClose).
      this._readyState = 1 // OPEN
    }

    return result
  }

  _handleOpen(): void {
    this._readyState = 1
    if (this.onopen) {
      this.onopen.call(this)
    }
  }

  _handleMessage(data: string | ArrayBuffer): void {
    if (this.onmessage) {
      this.onmessage.call(this, { data })
    }
  }

  _handleClose(code: number, reason: string): void {
    this._readyState = 3
    if (this.onclose) {
      this.onclose.call(this, { code, reason, wasClean: code === 1000 })
    }
  }

  _handleError(message: string): void {
    if (this.onerror) {
      this.onerror.call(this, { message })
    }
  }
}

const webSocketConnections = new Map<string, ServerWebSocket>()

/**
 * Most recent callback registered via native `setWebSocketHandler`.
 *
 * The native side (`cpp/HybridHttpServer.cpp` `g_wsHandler`) holds a single global
 * callback (last write wins), while this module exposes two entry points:
 * `ConfigServer._setupWebSocketHandler()` and `setupWebSocketHandler()`. Storing
 * the raw callback allows `autoRestart` to re-register the active owner without
 * overwriting one with the other.
 */
let reinstallWsHandler: ((event: import('./HttpServer.nitro').WebSocketEvent) => void) | null = null

/**
 * Set the global WebSocket event handler.
 * @param handler Callback invoked for WebSocket lifecycle and message events.
 */
export function setupWebSocketHandler(handler: (ws: ServerWebSocket, event: import('./HttpServer.nitro').WebSocketEvent) => void): void {
  const eventHandler = (event: import('./HttpServer.nitro').WebSocketEvent) => {
    let ws = webSocketConnections.get(event.connectionId)

    if (event.type === 'open') {
      ws = new ServerWebSocket(event.connectionId)
      webSocketConnections.set(event.connectionId, ws)
      ws._handleOpen()
      handler(ws, event)
    } else if (ws) {
      switch (event.type) {
        case 'message':
          const data = event.binaryData || event.textData || ''
          ws._handleMessage(data)
          handler(ws, event)
          break
        case 'close':
          ws._handleClose(event.closeCode || 1000, event.closeReason || '')
          webSocketConnections.delete(event.connectionId)
          handler(ws, event)
          break
        case 'error':
          ws._handleError(event.errorMessage || 'Unknown error')
          handler(ws, event)
          break
      }
    }
  }

  HttpServerModule.setWebSocketHandler(eventHandler)
  // Track the most recently installed callback for autoRestart re-registration.
  reinstallWsHandler = eventHandler
}

/** Get all active WebSocket connections. */
export function getWebSocketConnections(): Map<string, ServerWebSocket> {
  return webSocketConnections
}

/** Get a specific WebSocket connection by ID. */
export function getWebSocket(connectionId: string): ServerWebSocket | undefined {
  return webSocketConnections.get(connectionId)
}

// Node.js compatible HTTP interface
export * from './http'
export { createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'

import { Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'
export default { createHttpServer, createStaticServer, createAppServer, createConfigServer, HttpServer, StaticServer, AppServer, ConfigServer, createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS, ServerWebSocket, setupWebSocketHandler, getWebSocketConnections, getWebSocket, setCorsConfig }