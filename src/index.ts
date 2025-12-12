import { NitroModules } from 'react-native-nitro-modules'
import type { HttpServer as NitroHttpServer, HttpRequest, HttpResponse as NitroHttpResponse, ServerConfig } from './HttpServer.nitro'
import { createServer } from 'http'

// Redefine HttpResponse for User (User sees unified body)
export interface HttpResponse extends Omit<NitroHttpResponse, 'body' | 'binaryBody'> {
  body?: string | ArrayBuffer
}

// Redefine RequestHandler to use local HttpResponse
export type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse

// 创建 HybridObject 实例
const HttpServerModule = NitroModules.createHybridObject<NitroHttpServer>("HttpServer")

// Helper function to wrap handler and intercept binary body
const wrapHandler = (handler: RequestHandler): (request: HttpRequest) => Promise<NitroHttpResponse> => {
  return async (request: HttpRequest) => {
    const response = await handler(request)

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
  }
}

// 普通 HTTP 服务器
export class HttpServer {
  private _isRunning = false

  async start(port: number, handler: RequestHandler, host?: string): Promise<boolean> {
    if (this._isRunning) {
      throw new Error('Server is already running')
    }

    const wrappedHandler = wrapHandler(handler)
    const success = await HttpServerModule.start(port, wrappedHandler, host)
    this._isRunning = success
    return success
  }

  async stop(): Promise<void> {
    if (!this._isRunning) return

    await HttpServerModule.stop()
    this._isRunning = false
  }

  async getStats(): Promise<Record<string, any>> {
    return await HttpServerModule.getStats()
  }

  isRunning(): boolean {
    return this._isRunning
  }
}

// 静态文件服务器
export class StaticServer {
  private _isRunning = false

  async start(port: number, rootDir: string, host?: string): Promise<boolean> {
    if (this._isRunning) {
      throw new Error('Static server is already running')
    }

    const success = await HttpServerModule.startStaticServer(port, rootDir, host)
    this._isRunning = success
    return success
  }

  async stop(): Promise<void> {
    if (!this._isRunning) return

    await HttpServerModule.stopStaticServer()
    this._isRunning = false
  }

  isRunning(): boolean {
    return this._isRunning
  }
}

// App HTTP 服务器 (混合静态文件和回调)
export class AppServer {
  private _isRunning = false

  async start(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<boolean> {
    if (this._isRunning) {
      throw new Error('App server is already running')
    }

    const wrappedHandler = wrapHandler(handler)
    const success = await HttpServerModule.startAppServer(port, rootDir, wrappedHandler, host)
    this._isRunning = success
    return success
  }

  async stop(): Promise<void> {
    if (!this._isRunning) return

    await HttpServerModule.stopAppServer()
    this._isRunning = false
  }

  isRunning(): boolean {
    return this._isRunning
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
  private _isRunning = false
  private _wsEnabled = false
  private _wsHandlers: Map<string, WebSocketConnectionHandler> = new Map()

  /**
   * 注册 WebSocket 连接处理器
   * @param path WebSocket 端点路径 (如 '/ws')
   * @param handler 连接处理器
   */
  onWebSocket(path: string, handler: WebSocketConnectionHandler): this {
    this._wsHandlers.set(path, handler)
    return this
  }

  async start(port: number, handler: RequestHandler, config: ServerConfig, host?: string): Promise<boolean> {
    if (this._isRunning) {
      throw new Error('Config server is already running')
    }

    // 检查是否有 WebSocket 配置
    if (config.mounts) {
      const wsMount = config.mounts.find((m: any) => m.type === 'websocket')
      if (wsMount) {
        this._wsEnabled = true
      }
    }

    const wrappedHandler = wrapHandler(handler)
    const configJson = JSON.stringify(config)
    const success = await HttpServerModule.startServerWithConfig(port, wrappedHandler, configJson, host)
    this._isRunning = success

    // 如果启动成功且有 WebSocket 配置，设置 WebSocket 处理器
    if (success && this._wsEnabled) {
      this._setupWebSocketHandler()
    }

    return success
  }

  private _setupWebSocketHandler(): void {
    // 使用闭包捕获 handlers，避免 this 绑定问题
    const wsHandlers = this._wsHandlers

    HttpServerModule.setWebSocketHandler((event) => {
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
    })
  }

  async stop(): Promise<void> {
    if (!this._isRunning) return

    await HttpServerModule.stopAppServer()
    this._isRunning = false
    this._wsEnabled = false
    this._wsHandlers.clear()
  }

  isRunning(): boolean {
    return this._isRunning
  }
}

/**
 * 创建并启动普通 HTTP 服务器
 * @param port 端口号
 * @param handler 请求处理器
 * @param host 监听的IP地址
 */
export async function createHttpServer(port: number, handler: RequestHandler, host?: string): Promise<HttpServer> {
  const server = new HttpServer()
  await server.start(port, handler, host)
  return server
}

/**
 * 创建并启动静态文件服务器
 * @param port 端口号
 * @param rootDir 静态文件根目录路径
 * @param host 监听的IP地址
 */
export async function createStaticServer(port: number, rootDir: string, host?: string): Promise<StaticServer> {
  const server = new StaticServer()
  await server.start(port, rootDir, host)
  return server
}

/**
 * 创建并启动 App HTTP 服务器
 * @param port 端口号
 * @param rootDir 静态文件根目录路径
 * @param handler 请求处理器
 * @param host 监听的IP地址
 */
export async function createAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<AppServer> {
  const server = new AppServer()
  await server.start(port, rootDir, handler, host)
  return server
}

/**
 * 创建并启动带配置的 App HTTP 服务器
 * @param port 端口号
 * @param handler 请求处理器
 * @param config 插件配置（可包含 root_dir 指定静态文件根目录）
 * @param host 监听的IP地址
 */
export async function createConfigServer(port: number, handler: RequestHandler, config: ServerConfig, host?: string): Promise<ConfigServer> {
  const server = new ConfigServer()
  await server.start(port, handler, config, host)
  return server
}

// 导出类型和实例
export type { HttpRequest, ServerConfig, DirListConfig, Mountable, WebDavMount, ZipMount, StaticMount, UploadMount, BufferUploadMount, RewriteMount, RewriteRule, WebSocketMount, WebSocketEvent, WebSocketEventType, WebSocketHandler } from './HttpServer.nitro'

export { HttpServerModule }

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

  /** 发送文本消息 */
  async send(data: string | ArrayBuffer): Promise<boolean> {
    if (this._readyState !== 1) {
      throw new Error('WebSocket is not open')
    }

    if (typeof data === 'string') {
      return await HttpServerModule.wsSendText(this._connectionId, data)
    } else {
      return await HttpServerModule.wsSendBinary(this._connectionId, data)
    }
  }

  /** 关闭连接 */
  async close(code: number = 1000, reason: string = ''): Promise<boolean> {
    if (this._readyState >= 2) {
      return false
    }

    this._readyState = 2 // CLOSING
    const result = await HttpServerModule.wsClose(this._connectionId, code, reason)
    this._readyState = 3 // CLOSED
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
 * 设置 WebSocket 事件处理器
 * @param handler 处理 WebSocket 事件的回调函数
 */
export function setupWebSocketHandler(handler: (ws: ServerWebSocket, event: import('./HttpServer.nitro').WebSocketEvent) => void): void {
  HttpServerModule.setWebSocketHandler((event) => {
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
  })
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
export default { createHttpServer, createStaticServer, createAppServer, createConfigServer, HttpServer, StaticServer, AppServer, ConfigServer, createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS, ServerWebSocket, setupWebSocketHandler, getWebSocketConnections, getWebSocket }