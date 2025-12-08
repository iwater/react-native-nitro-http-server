import { type HybridObject } from 'react-native-nitro-modules';
export interface HttpRequest {
    requestId: string;
    method: string;
    path: string;
    headers: Record<string, string>;
    body?: string;
}
export interface HttpResponse {
    statusCode: number;
    headers?: Record<string, string>;
    body?: string;
}
export interface ServerStats {
    totalRequests: number;
    activeConnections: number;
    bytesSent: number;
    bytesReceived: number;
    uptime: number;
    errorCount: number;
}
export type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse;
export interface HttpServer extends HybridObject<{
    ios: 'swift';
    android: 'kotlin';
}> {
    /**
     * 启动 HTTP 服务器
     * @param port 端口号
     * @param handler 请求处理器
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    start(port: number, handler: RequestHandler, host?: string): Promise<boolean>;
    /**
     * 发送 HTTP 响应
     * @param requestId 请求ID
     * @param response HTTP 响应
     * @returns 是否发送成功
     */
    sendResponse(requestId: string, response: HttpResponse): Promise<boolean>;
    /**
     * 停止 HTTP 服务器
     */
    stop(): Promise<void>;
    /**
     * 获取服务器统计信息
     * @returns 统计信息
     */
    getStats(): Promise<ServerStats>;
    /**
     * 获取当前是否正在运行
     * @returns 服务器是否在运行
     */
    isRunning(): Promise<boolean>;
    /**
     * 启动静态文件服务器
     * @param port 端口号
     * @param rootDir 静态文件根目录路径
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    startStaticServer(port: number, rootDir: string, host?: string): Promise<boolean>;
    /**
     * 停止静态文件服务器
     */
    stopStaticServer(): Promise<void>;
    /**
     * 分块读取请求体
     * @param requestId 请求 ID
     * @returns 读取的数据块（空字符串表示结束）
     */
    readRequestBodyChunk(requestId: string): Promise<string>;
    /**
     * 分块写入响应体
     * @param requestId 请求 ID
     * @param chunk 数据块
     * @returns 是否写入成功
     */
    writeResponseChunk(requestId: string, chunk: string): Promise<boolean>;
    /**
     * 结束响应
     * @param requestId 请求 ID
     * @param statusCode HTTP 状态码
     * @param headersJson 响应头 JSON 字符串
     * @returns 是否成功
     */
    endResponse(requestId: string, statusCode: number, headersJson: string): Promise<boolean>;
    /**
     * 启动App HTTP服务器（混合静态文件和回调）
     * @param port 端口号
     * @param rootDir 静态文件根目录路径
     * @param handler 请求处理器
     * @param host 监听的IP地址,默认为 127.0.0.1
     * @returns 是否启动成功
     */
    startAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<boolean>;
    /**
     * 停止App HTTP服务器
     */
    stopAppServer(): Promise<void>;
}
