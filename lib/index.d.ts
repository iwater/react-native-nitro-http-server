import type { HttpServer, HttpRequest, HttpResponse, RequestHandler } from './HttpServer.nitro';
import { createServer } from 'http';
declare const HttpServerModule: HttpServer;
export declare class ReactNativeHttpServer {
    private isRunning;
    private isStaticServerRunning;
    private isAppServerRunning;
    private currentHandler?;
    start(port: number, handler: RequestHandler, host?: string): Promise<boolean>;
    stop(): Promise<void>;
    getStats(): Promise<Record<string, any>>;
    isServerRunning(): boolean;
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
     * 检查静态服务器是否正在运行
     */
    isStaticRunning(): boolean;
    /**
     * 便捷方法：创建并启动静态文件服务器
     * @param port 端口号
     * @param staticDir 静态文件根目录路径
     * @returns ReactNativeHttpServer 实例
     */
    static createStaticServer(port: number, staticDir: string, host?: string): Promise<ReactNativeHttpServer>;
    /**
     * 启动App HTTP服务器(混合静态文件和回调)
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
    /**
     * 检查App服务器是否正在运行
     */
    isAppRunning(): boolean;
    /**
     * 便捷方法：创建并启动App HTTP服务器
     * @param port 端口号
     * @param staticDir 静态文件根目录路径
     * @returns ReactNativeHttpServer 实例
     */
    static createAppServer(port: number, staticDir: string, handler: RequestHandler, host?: string): Promise<ReactNativeHttpServer>;
}
export type { HttpRequest, HttpResponse, RequestHandler };
export { HttpServerModule };
export * from './http';
export { createServer, Server, IncomingMessage, ServerResponse, STATUS_CODES, METHODS } from './http';
import { Server, IncomingMessage, ServerResponse } from './http';
declare const _default: {
    ReactNativeHttpServer: typeof ReactNativeHttpServer;
    createServer: typeof createServer;
    Server: typeof Server;
    IncomingMessage: typeof IncomingMessage;
    ServerResponse: typeof ServerResponse;
    STATUS_CODES: Record<number, string>;
    METHODS: string[];
};
export default _default;
