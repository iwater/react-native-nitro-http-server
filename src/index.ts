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

// 带配置的 App HTTP 服务器 (支持 WebDAV、Zip 挂载等插件)
export class ConfigServer {
  private _isRunning = false

  async start(port: number, rootDir: string, handler: RequestHandler, config: ServerConfig, host?: string): Promise<boolean> {
    if (this._isRunning) {
      throw new Error('Config server is already running')
    }

    const wrappedHandler = wrapHandler(handler)
    const configJson = JSON.stringify(config)
    const success = await HttpServerModule.startServerWithConfig(port, rootDir, wrappedHandler, configJson, host)
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
 * @param rootDir 静态文件根目录路径
 * @param handler 请求处理器
 * @param config 插件配置
 * @param host 监听的IP地址
 */
export async function createConfigServer(port: number, rootDir: string, handler: RequestHandler, config: ServerConfig, host?: string): Promise<ConfigServer> {
  const server = new ConfigServer()
  await server.start(port, rootDir, handler, config, host)
  return server
}

// 导出类型和实例
export type { HttpRequest, ServerConfig, DirListConfig } from './HttpServer.nitro'
export { HttpServerModule }

// Node.js 兼容的 HTTP 接口
export * from './http'
export { createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'

import { Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'
export default { createHttpServer, createStaticServer, createAppServer, createConfigServer, HttpServer, StaticServer, AppServer, ConfigServer, createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS }