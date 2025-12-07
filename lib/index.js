"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) __createBinding(exports, m, p);
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.METHODS = exports.STATUS_CODES = exports.ServerResponse = exports.IncomingMessage = exports.Server = exports.createServer = exports.HttpServerModule = exports.ReactNativeHttpServer = void 0;
// src/index.ts
const react_native_nitro_modules_1 = require("react-native-nitro-modules");
const http_1 = require("http");
// 创建 HybridObject 实例
const HttpServerModule = react_native_nitro_modules_1.NitroModules.createHybridObject("HttpServer");
exports.HttpServerModule = HttpServerModule;
// 高级封装
class ReactNativeHttpServer {
    constructor() {
        this.isRunning = false;
        this.isStaticServerRunning = false;
    }
    async start(port, handler) {
        if (this.isRunning) {
            throw new Error('Server is already running');
        }
        this.currentHandler = handler;
        const success = await HttpServerModule.start(port, handler);
        this.isRunning = success;
        return success;
    }
    async stop() {
        if (!this.isRunning)
            return;
        await HttpServerModule.stop();
        this.isRunning = false;
        this.currentHandler = undefined;
    }
    async getStats() {
        return await HttpServerModule.getStats();
    }
    isServerRunning() {
        return this.isRunning;
    }
    /**
     * 启动静态文件服务器
     * @param port 端口号
     * @param rootDir 静态文件根目录路径
     * @returns 是否启动成功
     */
    async startStaticServer(port, rootDir) {
        if (this.isStaticServerRunning) {
            throw new Error('Static server is already running');
        }
        const success = await HttpServerModule.startStaticServer(port, rootDir);
        this.isStaticServerRunning = success;
        return success;
    }
    /**
     * 停止静态文件服务器
     */
    async stopStaticServer() {
        if (!this.isStaticServerRunning)
            return;
        await HttpServerModule.stopStaticServer();
        this.isStaticServerRunning = false;
    }
    /**
     * 检查静态服务器是否正在运行
     */
    isStaticRunning() {
        return this.isStaticServerRunning;
    }
    /**
     * 便捷方法：创建并启动静态文件服务器
     * @param port 端口号
     * @param staticDir 静态文件根目录路径
     * @returns ReactNativeHttpServer 实例
     */
    static async createStaticServer(port, staticDir) {
        const server = new ReactNativeHttpServer();
        await server.startStaticServer(port, staticDir);
        return server;
    }
}
exports.ReactNativeHttpServer = ReactNativeHttpServer;
// export default ReactNativeHttpServer
// Node.js 兼容的 HTTP 接口
__exportStar(require("./http"), exports);
var http_2 = require("./http");
Object.defineProperty(exports, "createServer", { enumerable: true, get: function () { return http_2.createServer; } });
Object.defineProperty(exports, "Server", { enumerable: true, get: function () { return http_2.Server; } });
Object.defineProperty(exports, "IncomingMessage", { enumerable: true, get: function () { return http_2.IncomingMessage; } });
Object.defineProperty(exports, "ServerResponse", { enumerable: true, get: function () { return http_2.ServerResponse; } });
Object.defineProperty(exports, "STATUS_CODES", { enumerable: true, get: function () { return http_2.STATUS_CODES; } });
Object.defineProperty(exports, "METHODS", { enumerable: true, get: function () { return http_2.METHODS; } });
const http_3 = require("./http");
exports.default = { ReactNativeHttpServer, createServer: http_1.createServer, Server: http_3.Server, IncomingMessage: http_3.IncomingMessage, ServerResponse: http_3.ServerResponse, STATUS_CODES: http_3.STATUS_CODES, METHODS: http_3.METHODS };
