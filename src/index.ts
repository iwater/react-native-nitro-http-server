// src/index.ts
import { NitroModules } from 'react-native-nitro-modules'
import type { HttpServer, HttpRequest, HttpResponse as NitroHttpResponse } from './HttpServer.nitro'
import { createServer } from 'http'

// Redefine HttpResponse for User (User sees unified body)
export interface HttpResponse extends Omit<NitroHttpResponse, 'body' | 'binaryBody'> {
    body?: string | ArrayBuffer
}

// Redefine RequestHandler to use local HttpResponse
export type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse

// 创建 HybridObject 实例
const HttpServerModule = NitroModules.createHybridObject<HttpServer>("HttpServer")

// 高级封装
export class ReactNativeHttpServer {
    private isRunning = false
    private isStaticServerRunning = false
    private isAppServerRunning = false
    private currentHandler?: RequestHandler

    async start(port: number, handler: RequestHandler, host?: string): Promise<boolean> {
        if (this.isRunning) {
            throw new Error('Server is already running')
        }

        // Wrap the handler to intercept binaryBody
        const wrappedHandler: (request: HttpRequest) => Promise<NitroHttpResponse> = async (request) => {
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

        this.currentHandler = wrappedHandler
        const success = await HttpServerModule.start(port, wrappedHandler, host)
        this.isRunning = success
        return success
    }

    async stop(): Promise<void> {
        if (!this.isRunning) return

        await HttpServerModule.stop()
        this.isRunning = false
        this.currentHandler = undefined
    }

    async getStats(): Promise<Record<string, any>> {
        return await HttpServerModule.getStats()
    }

    isServerRunning(): boolean {
        return this.isRunning
    }

    /**
     * 启动静态文件服务器
     * @param port 端口号
     * @param rootDir 静态文件根目录路径
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    async startStaticServer(port: number, rootDir: string, host?: string): Promise<boolean> {
        if (this.isStaticServerRunning) {
            throw new Error('Static server is already running')
        }

        const success = await HttpServerModule.startStaticServer(port, rootDir, host)
        this.isStaticServerRunning = success
        return success
    }

    /**
     * 停止静态文件服务器
     */
    async stopStaticServer(): Promise<void> {
        if (!this.isStaticServerRunning) return

        await HttpServerModule.stopStaticServer()
        this.isStaticServerRunning = false
    }

    /**
     * 检查静态服务器是否正在运行
     */
    isStaticRunning(): boolean {
        return this.isStaticServerRunning
    }

    /**
     * 便捷方法：创建并启动静态文件服务器
     * @param port 端口号
     * @param staticDir 静态文件根目录路径
     * @returns ReactNativeHttpServer 实例
     */
    static async createStaticServer(port: number, staticDir: string, host?: string): Promise<ReactNativeHttpServer> {
        const server = new ReactNativeHttpServer()
        await server.startStaticServer(port, staticDir, host)
        return server
    }

    /**
     * 启动App HTTP服务器(混合静态文件和回调)
     * @param port 端口号
     * @param rootDir 静态文件根目录路径
     * @param handler 请求处理器
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    async startAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<boolean> {
        if (this.isAppServerRunning) {
            throw new Error('Server is already running')
        }

        // Wrap the handler to intercept binaryBody
        const wrappedHandler: (request: HttpRequest) => Promise<NitroHttpResponse> = async (request) => {
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

        this.currentHandler = wrappedHandler
        const success = await HttpServerModule.startAppServer(port, rootDir, wrappedHandler, host)
        this.isAppServerRunning = success
        return success
    }

    /**
     * 停止App HTTP服务器
     */
    async stopAppServer(): Promise<void> {
        if (!this.isAppServerRunning) return

        await HttpServerModule.stopAppServer()
        this.isAppServerRunning = false
        this.currentHandler = undefined
    }

    /**
     * 检查App服务器是否正在运行
     */
    isAppRunning(): boolean {
        return this.isAppServerRunning
    }

    /**
     * 便捷方法：创建并启动App HTTP服务器
     * @param port 端口号
     * @param staticDir 静态文件根目录路径
     * @returns ReactNativeHttpServer 实例
     */
    static async createAppServer(port: number, staticDir: string, handler: RequestHandler, host?: string): Promise<ReactNativeHttpServer> {
        const server = new ReactNativeHttpServer()
        await server.startAppServer(port, staticDir, handler, host)
        return server
    }

    /**
     * 发送二进制响应（线程安全）
     * 此方法在 JS 线程上同步复制 ArrayBuffer 数据，避免跨线程访问问题
     * 适用于 async handler 中需要返回二进制数据的场景
     * 
     * @param requestId 请求 ID
     * @param statusCode HTTP 状态码
     * @param headers 响应头
     * @param body 二进制响应体
     * @returns 是否发送成功
     * 
     * @example
     * server.start(8080, async (request) => {
     *   const imageBuffer = new ArrayBuffer(100);
     *   await server.sendBinaryResponse(
     *     request.requestId,
     *     200,
     *     { 'Content-Type': 'image/png' },
     *     imageBuffer
     *   );
     *   return { statusCode: 200 }; // 返回值会被忽略
     * });
     */

}

// 导出类型和实例
// 导出类型和实例
export type { HttpRequest }
export { HttpServerModule }
// export default ReactNativeHttpServer

// Node.js 兼容的 HTTP 接口
export * from './http'
export { createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'

import { Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'
export default { ReactNativeHttpServer, createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS }