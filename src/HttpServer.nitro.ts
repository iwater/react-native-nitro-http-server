// src/HttpServer.nitro.ts
import { type HybridObject } from 'react-native-nitro-modules'

// HTTP 请求接口
export interface HttpRequest {
    requestId: string
    method: string
    path: string
    headers: Record<string, string>
    body?: string
}

// HTTP 响应接口
export interface HttpResponse {
    statusCode: number
    headers?: Record<string, string>
    body?: string
}

// 服务器统计信息接口
export interface ServerStats {
    totalRequests: number
    activeConnections: number
    bytesSent: number
    bytesReceived: number
    uptime: number
    errorCount: number
}

// 请求处理器类型
export type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse

// HTTP 服务器接口
export interface HttpServer extends HybridObject<{
    ios: 'swift',
    android: 'kotlin'
}> {
    /**
     * 启动 HTTP 服务器
     * @param port 端口号
     * @param handler 请求处理器
     * @returns 是否启动成功
     */
    start(port: number, handler: RequestHandler): Promise<boolean>

    /**
     * 发送 HTTP 响应
     * @param requestId 请求ID
     * @param response HTTP 响应
     * @returns 是否发送成功
     */
    sendResponse(requestId: string, response: HttpResponse): Promise<boolean>

    /**
     * 停止 HTTP 服务器
     */
    stop(): Promise<void>

    /**
     * 获取服务器统计信息
     * @returns 统计信息
     */
    getStats(): Promise<ServerStats>

    /**
     * 获取当前是否正在运行
     * @returns 服务器是否在运行
     */
    isRunning(): Promise<boolean>

    /**
     * 启动静态文件服务器
     * @param port 端口号
     * @param rootDir 静态文件根目录路径
     * @returns 是否启动成功
     */
    startStaticServer(port: number, rootDir: string): Promise<boolean>

    /**
     * 停止静态文件服务器
     */
    stopStaticServer(): Promise<void>
}