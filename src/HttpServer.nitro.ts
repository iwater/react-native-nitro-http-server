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
    /** 自定义响应头（对 rewrite/websocket/upload mount 为无操作） */
    headers?: Record<string, string>
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

// WebSocket 挂载
export interface WebSocketMount extends BaseMount {
    type: 'websocket'
    max_message_size?: number  // 最大消息大小（字节），默认 64MB
}

export type Mountable = WebDavMount | ZipMount | StaticMount | UploadMount | BufferUploadMount | RewriteMount | WebSocketMount

// CORS 配置
export interface CorsConfig {
    origin?: string          // 默认 "*"
    methods?: string[]       // 默认常见方法 + WebDAV 方法
    headers?: string[]       // 默认 ["*"]，preflight 时优先回显 Access-Control-Request-Headers
    credentials?: boolean    // 默认 false；为 true 时回显请求 Origin
    max_age?: number         // preflight 缓存秒数
}

// 服务器插件配置
export interface ServerConfig {
    root_dir?: string                       // 静态文件根目录（可选，作为默认静态挂载点）
    verbose?: boolean | 'off' | 'error' | 'warn' | 'info' | 'debug'  // 日志等级
    mime_types?: Record<string, string>     // 自定义 MIME types
    mounts?: Mountable[]                    // 统一挂载列表
    /** CORS 配置：true 启用默认（Allow-Origin: *），对象自定义，缺省关闭 */
    cors?: boolean | CorsConfig
    /**
     * 等 JS 回调返回响应的最长时间（秒），默认 30。
     * 长轮询 / SSE 场景必须调大，否则连接会被硬切断并返回 500。
     *
     * 注意这是**全局**状态（server 本身是全局单例）：只有显式传本字段才会改，
     * 重启 config server 时会重新应用。
     */
    request_timeout_secs?: number
    /**
     * 回调路径的请求体上限（字节），默认 100MB。
     *
     * 超过就返回 **413** 且**不回调 JS**（拒绝发生在 handler 之前）。大文件上传
     * 场景需要调大；注意插件路径（static / zip / upload / webdav 等）不读 body，
     * 不受这个上限约束 —— 只有落到用户 handler 的请求才受限。
     *
     * 同样是**全局**状态：只有显式传本字段才会改，重启 config server 时会重新应用。
     */
    max_body_size?: number
}

// WebSocket 事件类型
export type WebSocketEventType = 'open' | 'message' | 'close' | 'error'

// WebSocket 事件接口
export interface WebSocketEvent {
    connectionId: string           // 连接 ID
    type: WebSocketEventType       // 事件类型
    path?: string                  // 连接路径
    query?: string                 // 查询字符串 (仅 open 事件)
    headersJson?: string           // HTTP 头 JSON (仅 open 事件)
    textData?: string              // 文本消息数据
    binaryData?: ArrayBuffer       // 二进制消息数据
    closeCode?: number             // 关闭代码
    closeReason?: string           // 关闭原因
    errorMessage?: string          // 错误信息
}

// WebSocket 事件处理器类型
export type WebSocketHandler = (event: WebSocketEvent) => void

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
     * @returns 实际监听的端口号（0表示失败）
     */
    start(port: number, handler: RequestHandler, host?: string): Promise<number>

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
     * @returns 实际监听的端口号（0表示失败）
     */
    startStaticServer(port: number, rootDir: string, host?: string): Promise<number>

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
     * 设置全局 CORS 配置（对所有 server 类型生效，应在 server 启动前调用）
     * @param corsJson CORS 配置 JSON 字符串："true" 启用默认配置，对象可包含
     *   origin/methods/headers/credentials/max_age 字段，"false" 关闭
     */
    setCorsConfig(corsJson: string): void

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
     * @returns 实际监听的端口号（0表示失败）
     */
    startAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<number>

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
     * @returns 实际监听的端口号（0表示失败）
     */
    startServerWithConfig(port: number, handler: RequestHandler, configJson: string, host?: string): Promise<number>

    // ==================== 请求中断通知 ====================

    /**
     * 设置「请求被客户端中断」处理器。
     *
     * 客户端在 handler 还没返回响应时断开连接（关标签页 / 取消请求 / 网络断了），
     * 原生侧会**丢弃 handler future**，此前 JS 侧完全收不到通知 —— 只能在发送响应
     * 失败或等到 `request_timeout_secs` 超时时才间接察觉。
     *
     * 现在注册这个处理器就能在断开的那一刻拿到 `requestId`（回调自动切回 JS 线程）。
     * 典型用法是中止这个请求正在等的下游操作（DB 查询 / 上游 fetch）。
     *
     * ⚠️ 与 `setWebSocketHandler` 一样是**全局单回调，后装者胜**。
     * ⚠️ 只在「响应从未发出」时触发 —— 正常回完、超时、handler 抛异常都不会触发。
     *
     * @param handler 收到被中断请求的 requestId
     */
    setRequestAbortedHandler(handler: (requestId: string) => void): void

    // ==================== WebSocket API ====================

    /**
     * 设置 WebSocket 事件处理器
     * @param handler WebSocket 事件处理器
     */
    setWebSocketHandler(handler: WebSocketHandler): void

    /**
     * 发送 WebSocket 文本消息
     * @param connectionId 连接 ID
     * @param message 文本消息
     * @returns 是否发送成功
     */
    wsSendText(connectionId: string, message: string): Promise<boolean>

    /**
     * 发送 WebSocket 二进制消息
     * @param connectionId 连接 ID
     * @param data 二进制数据
     * @returns 是否发送成功
     */
    wsSendBinary(connectionId: string, data: ArrayBuffer): Promise<boolean>

    /**
     * 关闭 WebSocket 连接
     * @param connectionId 连接 ID
     * @param code 关闭代码 (默认 1000)
     * @param reason 关闭原因
     * @returns 是否关闭成功
     */
    wsClose(connectionId: string, code?: number, reason?: string): Promise<boolean>
}