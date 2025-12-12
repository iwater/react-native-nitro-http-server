// src/HttpServer.nitro.ts
import { type HybridObject } from 'react-native-nitro-modules'

// HTTP 请求接口
export interface HttpRequest {
    requestId: string
    method: string
    path: string
    headers: Record<string, string>
    body?: string
    binaryBody?: ArrayBuffer
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

// 基础挂载接口
interface BaseMount {
    path: string
}

// WebDAV 挂载
export interface WebDavMount extends BaseMount {
    type: 'webdav'
    root: string
}

// Zip 挂载
export interface ZipMount extends BaseMount {
    type: 'zip'
    zip_file: string
}

// 目录列表配置
export interface DirListConfig {
    enabled: boolean
    show_hidden?: boolean
}

// 静态文件挂载
export interface StaticMount extends BaseMount {
    type: 'static'
    root: string
    dir_list?: DirListConfig
    default_index?: string[]
}

// 上传插件挂载
export interface UploadMount extends BaseMount {
    type: 'upload'
    temp_dir: string
}

// Buffer 上传插件挂载 (将文件内容作为 ArrayBuffer 传递到 JS,最大支持 100MB)
export interface BufferUploadMount extends BaseMount {
    type: 'buffer_upload'
}

// 重写规则
export interface RewriteRule {
    pattern: string      // 正则表达式模式
    replacement: string  // 替换目标（支持 $1, $2 等捕获组）
}

// Rewrite 挂载
export interface RewriteMount {
    type: 'rewrite'
    rules: RewriteRule[]
}

export type Mountable = WebDavMount | ZipMount | StaticMount | UploadMount | BufferUploadMount | RewriteMount

// 服务器插件配置
export interface ServerConfig {
    root_dir?: string                       // 静态文件根目录（可选，作为默认静态挂载点）
    verbose?: boolean | 'off' | 'error' | 'warn' | 'info' | 'debug'  // 日志等级
    mime_types?: Record<string, string>     // 自定义 MIME types
    mounts?: Mountable[]                    // 统一挂载列表
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
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    start(port: number, handler: RequestHandler, host?: string): Promise<boolean>

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
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    startStaticServer(port: number, rootDir: string, host?: string): Promise<boolean>

    /**
     * 停止静态文件服务器
     */
    stopStaticServer(): Promise<void>

    /**
     * 分块读取请求体
     * @param requestId 请求 ID
     * @returns 读取的数据块（空字符串表示结束）
     */
    readRequestBodyChunk(requestId: string): Promise<string>

    /**
     * 分块写入响应体
     * @param requestId 请求 ID
     * @param chunk 数据块
     * @returns 是否写入成功
     */
    writeResponseChunk(requestId: string, chunk: string): Promise<boolean>

    /**
     * 结束响应
     * @param requestId 请求 ID
     * @param statusCode HTTP 状态码
     * @param headersJson 响应头 JSON 字符串
     * @returns 是否成功
     */
    endResponse(requestId: string, statusCode: number, headersJson: string): Promise<boolean>

    /**
     * 写入二进制响应体并发送响应（同步复制数据）
     * 此方法在 JS 线程上安全地复制 ArrayBuffer 数据，避免跨线程访问问题
     * @param requestId 请求 ID
     * @param statusCode HTTP 状态码
     * @param headersJson 响应头 JSON 字符串
     * @param body 二进制响应体
     * @returns 是否成功
     */
    sendBinaryResponse(requestId: string, statusCode: number, headersJson: string, body: ArrayBuffer): Promise<boolean>

    /**
     * 启动App HTTP服务器（混合静态文件和回调）
     * @param port 端口号
     * @param rootDir 静态文件根目录路径
     * @param handler 请求处理器
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    startAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<boolean>

    /**
     * 停止App HTTP服务器
     */
    stopAppServer(): Promise<void>

    /**
     * 启动带配置的App HTTP服务器（支持插件如 WebDAV、Zip 挂载）
     * @param port 端口号
     * @param handler 请求处理器
     * @param configJson 插件配置 JSON 字符串（可包含 root_dir 指定静态文件根目录）
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    startServerWithConfig(port: number, handler: RequestHandler, configJson: string, host?: string): Promise<boolean>
}