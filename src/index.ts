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
    private currentHandler?: RequestHandler

    async start(port: number, handler: RequestHandler): Promise<boolean> {
        if (this.isRunning) {
            throw new Error('Server is already running')
        }

        this.currentHandler = handler
        const success = await HttpServerModule.start(port, handler)
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
     * @returns 是否启动成功
     */
    async startStaticServer(port: number, rootDir: string): Promise<boolean> {
        if (this.isStaticServerRunning) {
            throw new Error('Static server is already running')
        }

        const success = await HttpServerModule.startStaticServer(port, rootDir)
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
    static async createStaticServer(port: number, staticDir: string): Promise<ReactNativeHttpServer> {
        const server = new ReactNativeHttpServer()
        await server.startStaticServer(port, staticDir)
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