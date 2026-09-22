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
 * 交给用户 handler 的请求对象 = 原生 `HttpRequest` + 本模块补的 `signal`。
 *
 * ⚠️ `signal` 是**本模块的扩展**，不是 Node 的：Node 22 的 `http.IncomingMessage`
 * 上**没有** `signal`（实测 `'signal' in req === false`），只有 `aborted` 属性与
 * `'aborted'` 事件。这里额外给一个 `AbortSignal` 是为了能直接喂给
 * `fetch(url, { signal })` / 下游 SDK 的取消参数。
 *
 * ⚠️ 宿主没有 `AbortController` 时（quickjs 的两个 headless runner）它是 `undefined`
 * —— 见 `requestAbort.ts` 的文件头。用之前判一下。
 */
export interface HttpRequestWithSignal extends HttpRequest {
  /** 客户端断开（响应发出前）时被 abort */
  readonly signal?: AbortSignal
}

// Redefine RequestHandler to use local HttpResponse
export type RequestHandler = (request: HttpRequestWithSignal) => Promise<HttpResponse> | HttpResponse

// 启动选项
export interface ServerOptions {
  /** 监听的 IP 地址，默认 127.0.0.1 */
  host?: string
  /**
   * 锁屏/切换App后回到前台时，自动检测并重启被系统挂起的服务。
   * 默认 false，设为 true 后无需在 App 中添加额外代码。
   */
  autoRestart?: boolean
}

// 兼容旧版 api：将 host 字符串或 options 对象统一为 ServerOptions
function normalizeOptions(hostOrOptions?: string | ServerOptions): ServerOptions {
  if (typeof hostOrOptions === 'string') {
    return { host: hostOrOptions }
  }
  return hostOrOptions || {}
}

// 创建 HybridObject 实例
const HttpServerModule = NitroModules.createHybridObject<NitroHttpServer>("HttpServer")

// Helper function to wrap handler and intercept binary body
const wrapHandler = (handler: RequestHandler): (request: HttpRequest) => Promise<NitroHttpResponse> => {
  // 把「客户端断开」的处理器装进原生侧（幂等）。放在这里而不是模块顶层：
  // 只在真正要起服务的那一刻才碰原生模块，纯 import 的用例不受影响。
  installRequestAbortedHandler(HttpServerModule)

  return async (request: HttpRequest) => {
    // 登记中断状态并给 request 挂 signal。
    // 时序要点：登记发生在**调用户 handler 之前**，所以 handler 里任何 await
    // 期间客户端断开都收得到通知；handler 落定后立刻摘掉。
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

// 普通 HTTP 服务器
export class HttpServer {
  /**
   * Node 的 handle-ref：running 的 server 顶住事件循环。
   * 宿主看不见原生 server（start() 是**真异步**的：await 原生 start 之后才
   * emit 'listening'），没有这个 ref 的话 loop 会先收泵、回调永远不投递。
   */
  private _loopRef = new LoopRef('http-server.HttpServer')
  private _isRunning = false
  private _port = 0
  private _handler: RequestHandler | null = null
  private _options: ServerOptions = {}
  private _appStateSub: { remove: () => void } | null = null
  private _intentionallyStopped = false

  async start(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<number> {
    // 等上一次原生 stop 落定：原生 server 是单例，停还没停完就起会撞车（见 nativeStopQueue.ts）
    await awaitPendingNativeStop()
    if (this._isRunning) {
      throw new Error('Server is already running')
    }

    this._options = normalizeOptions(hostOrOptions)
    this._handler = handler
    this._intentionallyStopped = false

    const wrappedHandler = wrapHandler(handler)
    const host = this._options.host
    // running 的 server 顶住 loop（Node 的 handle ref）。原生 start 是异步的，
    // 没有这个 ref 的话 loop 会在 'listening' 之前收泵。
    this._loopRef.acquire()
    let actualPort = 0
    try {
      actualPort = await HttpServerModule.start(port, wrappedHandler, host)
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
    return await HttpServerModule.getStats()
  }

  async isRunning(): Promise<boolean> {
    // 调用原生层的 isRunning()，内部会用 TCP connect 探测 socket 是否真的存活
    const nativeRunning = await HttpServerModule.isRunning()
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // 用 fetch 向自己的端口发 HEAD 请求做健康检查
    // 每个实例独立探测自己的端口，不受 C++ 层单例全局端口限制
    // 500ms 超时 + 失败 100ms 后重试一次：该请求会经过用户 handler，而回前台那一刻
    // JS 线程往往正忙 —— 20ms 会把**健康**服务器判死并重启它（重启窗口内连接全断）。
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
        // 必须清：失败路径（连接被拒时 fetch 立即 reject）不清就会留下悬挂的 500ms
        // 定时器 —— 每次探测一个，重试一次就是两个。
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
        // 这条路径绕过 stop()，所以自己配对 LoopRef：原生 stop 成功就放掉 ref
        // （handle 没了），重启成功再 acquire；重启失败 / 拿到端口 0 也放掉 ——
        // 否则 ref 一直挂着、进程再也退不出去。形状与 http.ts 的同款处理一致。
        // ⚠️ 本路径只在 RN 的 AppState 'active' 且服务已死时触发，无头宿主上跑不到，
        //    属防御性对称，R31 覆盖不到（R31 覆盖的是 start/stop 主路径）。
        try {
          // 清理可能的残留状态
          await HttpServerModule.stop()
          this._loopRef.release()
        } catch (_) { /* ignore */ }

        try {
          const wrappedHandler = wrapHandler(this._handler)
          const host = this._options.host
          this._loopRef.acquire()
          const actualPort = await HttpServerModule.start(this._port, wrappedHandler, host)
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

// 静态文件服务器
export class StaticServer {
  /** 同 HttpServer：running 的 server 顶住 loop，stop() 成功时释放。 */
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
    const nativeRunning = await HttpServerModule.isRunning()
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // 500ms 超时 + 失败 100ms 后重试一次：该请求会经过用户 handler，而回前台那一刻
    // JS 线程往往正忙 —— 20ms 会把**健康**服务器判死并重启它（重启窗口内连接全断）。
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
        // 必须清：失败路径（连接被拒时 fetch 立即 reject）不清就会留下悬挂的 500ms
        // 定时器 —— 每次探测一个，重试一次就是两个。
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
        // 同 HttpServer 的 auto-restart：绕过 stop()，ref 要自己配对（见上方注释）。
        try { await HttpServerModule.stopStaticServer(); this._loopRef.release() } catch (_) { /* ignore */ }

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

// App HTTP 服务器 (混合静态文件和回调)
export class AppServer {
  /** 同 HttpServer：running 的 server 顶住 loop，stop() 成功时释放。 */
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
    const nativeRunning = await HttpServerModule.isRunning()
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // 500ms 超时 + 失败 100ms 后重试一次：该请求会经过用户 handler，而回前台那一刻
    // JS 线程往往正忙 —— 20ms 会把**健康**服务器判死并重启它（重启窗口内连接全断）。
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
        // 必须清：失败路径（连接被拒时 fetch 立即 reject）不清就会留下悬挂的 500ms
        // 定时器 —— 每次探测一个，重试一次就是两个。
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
        // 同 HttpServer 的 auto-restart：绕过 stop()，ref 要自己配对（见上方注释）。
        try { await HttpServerModule.stopAppServer(); this._loopRef.release() } catch (_) { /* ignore */ }

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

// WebSocket 连接请求信息（包含握手信息）
export interface WebSocketConnectionRequest {
  path: string
  query: string
  headers: Record<string, string>
}

// WebSocket 连接处理器类型
export type WebSocketConnectionHandler = (ws: ServerWebSocket, request: WebSocketConnectionRequest) => void

// 带配置的 App HTTP 服务器 (支持 WebDAV、Zip 挂载、WebSocket 等插件)
export class ConfigServer {
  /** 同 HttpServer：running 的 server 顶住 loop，stop() 成功时释放。 */
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
   * 注册 WebSocket 连接处理器
   * @param path WebSocket 端点路径 (如 '/ws')
   * @param handler 连接处理器
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

    // 检查是否有 WebSocket 配置
    if (config.mounts) {
      const wsMount = config.mounts.find((m: any) => m.type === 'websocket')
      if (wsMount) {
        this._wsEnabled = true
      }
    }

    const wrappedHandler = wrapHandler(handler)
    const configJson = JSON.stringify(config)
    // 应用 CORS 配置（无配置时显式关闭，避免上次启动残留）
    // 注意：auto-restart 路径不需要重复调用 —— Rust 侧 CORS_CONFIG 是全局状态，stopAppServer 不会清除
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

    // 如果启动成功且有 WebSocket 配置，设置 WebSocket 处理器
    if (actualPort > 0 && this._wsEnabled) {
      this._setupWebSocketHandler()
    }

    if (this._options.autoRestart) {
      this._registerAutoRestart()
    }

    return actualPort
  }

  private _setupWebSocketHandler(): void {
    // 使用闭包捕获 handlers，避免 this 绑定问题
    const wsHandlers = this._wsHandlers

    const eventHandler = (event: import('./HttpServer.nitro').WebSocketEvent) => {
      let ws = webSocketConnections.get(event.connectionId)

      if (event.type === 'open') {
        // 创建 ServerWebSocket 实例
        ws = new ServerWebSocket(event.connectionId)
        webSocketConnections.set(event.connectionId, ws)

        // 解析 HTTP headers
        let headers: Record<string, string> = {}
        if (event.headersJson) {
          try {
            headers = JSON.parse(event.headersJson)
          } catch (e) {
            console.error('[WebSocket] Failed to parse headers JSON:', e)
          }
        }

        // 创建连接请求信息
        const request: WebSocketConnectionRequest = {
          path: event.path || '/ws',
          query: event.query || '',
          headers,
        }

        // 查找对应路径的处理器
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

        // 触发 onopen
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
    // 记住「最近一次装进原生侧的那个回调」：原生侧 setWebSocketHandler 是**全局单回调**
    // （后装者胜），autoRestart 后必须按同一归属原样装回去 —— 否则重启会把回调悄悄
    // 换给另一个来源（独立 setupWebSocketHandler 的处理器丢失，或反过来覆盖掉本类）。
    reinstallWsHandler = eventHandler
  }

  /**
   * 关闭并清理**模块级**连接表里的所有连接。
   *
   * 连接表（`webSocketConnections`）是模块级的，不清理就会一直持有已经断开的
   * `ServerWebSocket`（死连接泄漏），且下次 `start()` 时 `getWebSocketConnections()`
   * 会返回上一轮的对象。
   *
   * 为什么还要补发 `_handleClose`：native 的 close 事件要靠连接表查找才能回到对象上
   * （见 `_setupWebSocketHandler` 的 `else if (ws)`），条目被删掉之后它就永远不会到了
   * —— 于是 `onclose` 不触发、`readyState` 永久停在 OPEN/CLOSING（实测：stop() 之后
   * readyState 仍是 1）。这里在删除条目的那一刻补一次，与 `close()` 的语义一致
   * （「逻辑上已关闭」；真关闭由 native 事件再次置位，幂等）。
   */
  private async _closeAllWebSocketConnections(): Promise<void> {
    for (const [id, ws] of webSocketConnections) {
      try {
        await ws.close(1001, 'server stopped')
      } catch (_) { /* ignore */ }
      // delete() 返回 false = native 的 close 事件已抢先到达并删掉了条目 —— 不重复回调
      if (webSocketConnections.delete(id)) {
        ws._handleClose(1001, 'server stopped')
      }
    }
  }

  async stop(): Promise<void> {
    this._intentionallyStopped = true
    this._unregisterAutoRestart()

    // ⚠️ 清理连接要放在 `_isRunning` 提前返回**之前**：`isRunning()` 会把 `_isRunning`
    // 更新成原生侧的探测结果，所以「服务已经死了 → isRunning() 返回 false → stop()」
    // 这条**正常路径**会走提前返回。那时连接其实全都失效了，不清理就是漏一批。
    // 另外 `stop()` 走到这里时 `_intentionallyStopped` 已经置位，说明提前返回本来
    // 也不是「什么都不做」，只是「不需要再碰原生服务」。
    await this._closeAllWebSocketConnections()

    if (!this._isRunning) return

    await stopWithTracking(HttpServerModule.stopAppServer())
    this._loopRef.release()
    this._isRunning = false
    this._wsEnabled = false
    this._wsHandlers.clear()
  }

  async isRunning(): Promise<boolean> {
    const nativeRunning = await HttpServerModule.isRunning()
    this._isRunning = nativeRunning
    return nativeRunning
  }

  private async _probeAlive(): Promise<boolean> {
    // 500ms 超时 + 失败 100ms 后重试一次：该请求会经过用户 handler，而回前台那一刻
    // JS 线程往往正忙 —— 20ms 会把**健康**服务器判死并重启它（重启窗口内连接全断）。
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
        // 必须清：失败路径（连接被拒时 fetch 立即 reject）不清就会留下悬挂的 500ms
        // 定时器 —— 每次探测一个，重试一次就是两个。
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
        try { await HttpServerModule.stopAppServer() } catch (_) { /* ignore */ }

        try {
          const wrappedHandler = wrapHandler(this._handler)
          const configJson = JSON.stringify(this._config)
          const host = this._options.host
          const actualPort = await HttpServerModule.startServerWithConfig(this._port, wrappedHandler, configJson, host)
          if (actualPort > 0) {
            this._isRunning = true
            this._port = actualPort
            // 旧服务已停：连接表里的连接全部失效，摘掉（同 stop()；否则每次自动重启都漏一批）
            await this._closeAllWebSocketConnections()
            // 重挂 WebSocket 处理器。原生侧是全局单回调（后装者胜），必须按**重启前的
            // 归属**重挂：无条件调 this._setupWebSocketHandler() 会把独立
            // setupWebSocketHandler 注册的处理器挤掉（反过来也会覆盖掉 ConfigServer 的）。
            if (reinstallWsHandler) {
              HttpServerModule.setWebSocketHandler(reinstallWsHandler)
            } else if (this._wsEnabled) {
              this._setupWebSocketHandler()
            }
            console.log(`[ConfigServer] Auto-restarted on port ${actualPort}`)
          }
        } catch (e) {
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
 * 创建并启动普通 HTTP 服务器
 * @param port 端口号
 * @param handler 请求处理器
 * @param options 启动选项 (host, autoRestart)
 */
export async function createHttpServer(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<HttpServer> {
  const server = new HttpServer()
  await server.start(port, handler, hostOrOptions)
  return server
}

/**
 * 创建并启动静态文件服务器
 * @param port 端口号
 * @param rootDir 静态文件根目录路径
 * @param options 启动选项 (host, autoRestart)
 */
export async function createStaticServer(port: number, rootDir: string, hostOrOptions?: string | ServerOptions): Promise<StaticServer> {
  const server = new StaticServer()
  await server.start(port, rootDir, hostOrOptions)
  return server
}

/**
 * 创建并启动 App HTTP 服务器
 * @param port 端口号
 * @param rootDir 静态文件根目录路径
 * @param handler 请求处理器
 * @param options 启动选项 (host, autoRestart)
 */
export async function createAppServer(port: number, rootDir: string, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<AppServer> {
  const server = new AppServer()
  await server.start(port, rootDir, handler, hostOrOptions)
  return server
}

/**
 * 创建并启动带配置的 App HTTP 服务器
 * @param port 端口号
 * @param handler 请求处理器
 * @param config 插件配置（可包含 root_dir 指定静态文件根目录）
 * @param options 启动选项 (host, autoRestart)
 */
export async function createConfigServer(port: number, handler: RequestHandler, config: ServerConfig, hostOrOptions?: string | ServerOptions): Promise<ConfigServer> {
  const server = new ConfigServer()
  await server.start(port, handler, config, hostOrOptions)
  return server
}

/**
 * 设置全局 CORS 配置（对所有 server 类型生效，应在 server 启动前调用）
 * 注意：无效的 JSON 不会抛错，Rust 侧会记录警告并禁用 CORS
 * @param config true 启用默认（Allow-Origin: *），对象自定义，false 关闭
 */
export function setCorsConfig(config: boolean | CorsConfig): void {
  HttpServerModule.setCorsConfig(JSON.stringify(config ?? false))
}

// 导出类型和实例
export type { HttpRequest, ServerConfig, CorsConfig, DirListConfig, Mountable, WebDavMount, ZipMount, StaticMount, UploadMount, BufferUploadMount, RewriteMount, RewriteRule, WebSocketMount, WebSocketEvent, WebSocketEventType, WebSocketHandler } from './HttpServer.nitro'

export { HttpServerModule }

/** 当前登记中的「在飞请求」数量（诊断用；正常应回落到 0） */
export { pendingRequestCount } from './requestAbort'

// ==================== WebSocket API ====================

/**
 * ServerWebSocket - W3C WebSocket API 兼容的服务器端 WebSocket 接口
 * 用于在服务器端与客户端进行双向通信
 */
export class ServerWebSocket {
  private _connectionId: string
  private _readyState: number = 0 // 0=CONNECTING, 1=OPEN, 2=CLOSING, 3=CLOSED

  // W3C WebSocket 兼容的事件处理器
  public onopen: ((this: ServerWebSocket) => void) | null = null
  public onmessage: ((this: ServerWebSocket, event: { data: string | ArrayBuffer }) => void) | null = null
  public onclose: ((this: ServerWebSocket, event: { code: number; reason: string; wasClean: boolean }) => void) | null = null
  public onerror: ((this: ServerWebSocket, event: { message: string }) => void) | null = null

  constructor(connectionId: string) {
    this._connectionId = connectionId
    this._readyState = 1 // OPEN
  }

  /** 连接 ID */
  get connectionId(): string {
    return this._connectionId
  }

  /** W3C 兼容的 readyState 属性 */
  get readyState(): number {
    return this._readyState
  }

  /**
   * 发送文本或二进制消息
   *
   * 二进制接受 `ArrayBuffer` 或**任意视图**（TypedArray / DataView / nitro-buffer 的
   * `Buffer`）。原生侧 `wsSendBinary` 的参数类型是 `ArrayBuffer`，视图会被 Nitro 的
   * 转换器直接拒绝（实测报 "is not an ArrayBuffer! Are you maybe passing a TypedArray
   * (e.g. Uint8Array)? Try to pass its `.buffer` value."），所以这里先做归一化。
   */
  async send(data: string | ArrayBuffer | ArrayBufferView): Promise<boolean> {
    if (this._readyState !== 1) {
      throw new Error('WebSocket is not open')
    }

    if (typeof data === 'string') {
      return await HttpServerModule.wsSendText(this._connectionId, data)
    }

    // 与 wrapHandler 相同的 view → ArrayBuffer 归一化。
    // ⚠️ 不能无脑取 `.buffer`：Buffer / subarray 这类视图往往只覆盖底层 buffer 的一段，
    // 直接发 `.buffer` 会把**别人的字节**也发出去。只在「整段覆盖」时才复用底层 buffer。
    const buffer = data instanceof ArrayBuffer
      ? data
      : (data.byteLength === data.buffer.byteLength && data.byteOffset === 0)
        ? data.buffer
        : data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength)
    return await HttpServerModule.wsSendBinary(this._connectionId, buffer as ArrayBuffer)
  }

  /**
   * 关闭连接
   *
   * @returns true = 关闭命令已入队（排在它之前入队的消息之后执行）；
   *          false = 未入队（连接不存在 / 发送队列已满 / 连接已断开）。
   *          false 时连接**其实还开着**，可以重试。
   */
  async close(code: number = 1000, reason: string = ''): Promise<boolean> {
    if (this._readyState >= 2) {
      return false
    }

    this._readyState = 2 // CLOSING
    const result = await HttpServerModule.wsClose(this._connectionId, code, reason)

    if (result) {
      // 已入队：send_task 会在它前面排队的消息都发完之后才真正关闭。
      // 这里的 CLOSED 是「逻辑上不再可用」；真正的关闭由 native close 事件
      // 经 _handleClose 再次置位（幂等）。
      this._readyState = 3 // CLOSED
    } else {
      // 未入队 —— 连接并没有被关闭。若这里也置成 CLOSED，JS 侧会误以为关掉了，
      // 而连接其实还开着（且没人会再去关它）。回退成 OPEN 让调用方可以重试：
      // - 若连接真的已断开，随后的 send()/close() 会返回 false；
      // - native 的 close 事件到达时 _handleClose 仍会把状态置成 CLOSED。
      this._readyState = 1 // OPEN
    }

    return result
  }

  // 内部方法：处理事件
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

/** WebSocket 连接管理器 */
const webSocketConnections = new Map<string, ServerWebSocket>()

/**
 * 最近一次通过 `setWebSocketHandler` 装进原生侧的那个回调。
 *
 * 原生侧（`cpp/HybridHttpServer.cpp` 的 `g_wsHandler`）是**全局单回调，后装者胜**，
 * 而本模块有**两个**安装入口：`ConfigServer._setupWebSocketHandler()` 与独立的
 * `setupWebSocketHandler()`。autoRestart 重启后必须把「重启前生效的那一个」原样装回去，
 * 否则回调归属会被换掉：独立入口注册的处理器丢失（反之也会覆盖掉 ConfigServer 的）。
 *
 * 这里存**回调本体**而不是「谁来安装」，是为了让重挂不依赖实例、也不需要重跑安装逻辑。
 */
let reinstallWsHandler: ((event: import('./HttpServer.nitro').WebSocketEvent) => void) | null = null

/** 
 * 设置 WebSocket 事件处理器
 * @param handler 处理 WebSocket 事件的回调函数
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
  // 同 ConfigServer._setupWebSocketHandler：记住最近一次安装的回调，供 autoRestart 重挂
  reinstallWsHandler = eventHandler
}

/** 获取所有活跃的 WebSocket 连接 */
export function getWebSocketConnections(): Map<string, ServerWebSocket> {
  return webSocketConnections
}

/** 获取指定的 WebSocket 连接 */
export function getWebSocket(connectionId: string): ServerWebSocket | undefined {
  return webSocketConnections.get(connectionId)
}

// Node.js 兼容的 HTTP 接口
export * from './http'
export { createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'

import { Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'
export default { createHttpServer, createStaticServer, createAppServer, createConfigServer, HttpServer, StaticServer, AppServer, ConfigServer, createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS, ServerWebSocket, setupWebSocketHandler, getWebSocketConnections, getWebSocket, setCorsConfig }