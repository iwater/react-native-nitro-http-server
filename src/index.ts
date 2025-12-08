// src/index.ts
import { NitroModules } from 'react-native-nitro-modules'
import type { HttpServer, HttpRequest, HttpResponse, RequestHandler } from './HttpServer.nitro'
import { createServer } from 'http'

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

        this.currentHandler = handler
        const success = await HttpServerModule.start(port, handler, host)
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

        this.currentHandler = handler
        const success = await HttpServerModule.startAppServer(port, rootDir, handler, host)
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
}

// 导出类型和实例
export type { HttpRequest, HttpResponse, RequestHandler }
export { HttpServerModule }
// export default ReactNativeHttpServer

// Node.js 兼容的 HTTP 接口
export * from './http'
export { createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'

import { Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http'
export default { ReactNativeHttpServer, createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS }