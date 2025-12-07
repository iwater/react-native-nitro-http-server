import { EventEmitter } from 'events';
import type { HttpRequest, HttpResponse } from './HttpServer.nitro';
type RequestListener = (req: IncomingMessage, res: ServerResponse) => void;
interface ServerOptions {
    IncomingMessage?: typeof IncomingMessage;
    ServerResponse?: typeof ServerResponse;
    requestTimeout?: number;
    headersTimeout?: number;
    keepAliveTimeout?: number;
}
interface AddressInfo {
    address: string;
    family: string;
    port: number;
}
export declare const STATUS_CODES: Record<number, string>;
export declare const METHODS: string[];
/**
 * Node.js compatible IncomingMessage implementation
 * Implements a subset of http.IncomingMessage interface
 */
export declare class IncomingMessage extends EventEmitter {
    readonly method: string;
    readonly url: string;
    readonly headers: Record<string, string | string[] | undefined>;
    readonly rawHeaders: string[];
    readonly httpVersion: string;
    readonly httpVersionMajor: number;
    readonly httpVersionMinor: number;
    readable: boolean;
    complete: boolean;
    private _body;
    private _bodyEmitted;
    private _requestId;
    constructor(request: HttpRequest);
    /**
     * Convert headers to lowercase keys (Node.js convention)
     */
    private _normalizeHeaders;
    /**
     * Convert to rawHeaders format [key1, value1, key2, value2, ...]
     */
    private _toRawHeaders;
    /**
     * Emit body data (called internally after listeners are attached)
     */
    _emitBody(): void;
    read(_size?: number): Buffer | null;
    pause(): this;
    resume(): this;
    setEncoding(_encoding: string): this;
    destroy(_error?: Error): this;
    pipe<T extends NodeJS.WritableStream>(destination: T): T;
}
/**
 * Node.js compatible ServerResponse implementation
 * Implements a subset of http.ServerResponse interface
 */
export declare class ServerResponse extends EventEmitter {
    statusCode: number;
    statusMessage: string;
    headersSent: boolean;
    private _headers;
    private _body;
    private _ended;
    private _requestId;
    private _sendResponse;
    private _finished;
    writable: boolean;
    writableEnded: boolean;
    writableFinished: boolean;
    constructor(requestId: string, sendResponse: (response: HttpResponse) => Promise<boolean>);
    /**
     * Sets a single header value
     */
    setHeader(name: string, value: string | number | readonly string[]): this;
    /**
     * Gets a header value
     */
    getHeader(name: string): string | number | readonly string[] | undefined;
    /**
     * Returns header names
     */
    getHeaderNames(): string[];
    /**
     * Returns all headers as object
     */
    getHeaders(): Record<string, string | number | readonly string[] | undefined>;
    /**
     * Checks if a header exists
     */
    hasHeader(name: string): boolean;
    /**
     * Removes a header
     */
    removeHeader(name: string): void;
    /**
     * Sends response headers
     */
    writeHead(statusCode: number, statusMessage?: string | Record<string, string | number | readonly string[]>, headers?: Record<string, string | number | readonly string[]>): this;
    /**
     * Writes data to response body
     */
    write(chunk: string | Buffer, encodingOrCallback?: BufferEncoding | (() => void), callback?: () => void): boolean;
    /**
     * Ends the response
     */
    end(chunkOrCallback?: string | Buffer | (() => void), encodingOrCallback?: BufferEncoding | (() => void), callback?: () => void): this;
    /**
     * Flushes headers (no-op in our implementation)
     */
    flushHeaders(): void;
    /**
     * Cork/Uncork (no-op in our implementation)
     */
    cork(): void;
    uncork(): void;
    /**
     * Constructs and sends the response via native layer
     */
    private _sendNativeResponse;
    get finished(): boolean;
}
/**
 * Node.js compatible Server implementation
 * Implements a subset of http.Server interface
 */
export declare class Server extends EventEmitter {
    listening: boolean;
    private _port;
    private _requestListener?;
    private _nativeServer;
    private _options;
    constructor(options?: ServerOptions | RequestListener, requestListener?: RequestListener);
    /**
     * Starts listening for connections
     */
    listen(port: number, hostname?: string | (() => void), backlog?: number | (() => void), callback?: () => void): this;
    listen(port: number, callback?: () => void): this;
    /**
     * Stops the server
     */
    close(callback?: (err?: Error) => void): this;
    /**
     * Returns the bound address
     */
    address(): AddressInfo | string | null;
    /**
     * Handle incoming request from native layer
     */
    private _handleNativeRequest;
    setTimeout(_msecs?: number, _callback?: () => void): this;
    get timeout(): number;
    set timeout(_value: number);
    get headersTimeout(): number;
    set headersTimeout(value: number);
    get requestTimeout(): number;
    set requestTimeout(value: number);
    get keepAliveTimeout(): number;
    set keepAliveTimeout(value: number);
}
/**
 * Creates a new HTTP server
 * Compatible with Node.js http.createServer()
 *
 * @example
 * // Basic usage
 * const server = createServer((req, res) => {
 *   res.writeHead(200, { 'Content-Type': 'text/plain' });
 *   res.end('Hello World');
 * });
 * server.listen(3000);
 *
 * @example
 * // With Koa
 * import Koa from 'koa';
 * const app = new Koa();
 * const server = createServer(app.callback());
 * server.listen(3000);
 */
export declare function createServer(requestListener?: RequestListener): Server;
export declare function createServer(options: ServerOptions, requestListener?: RequestListener): Server;
export type { RequestListener, ServerOptions, AddressInfo };
