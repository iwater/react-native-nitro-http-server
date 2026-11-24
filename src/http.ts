// Node.js Compatible HTTP Interface for React Native

import { EventEmitter } from 'eventemitter3';
import { NitroModules } from 'react-native-nitro-modules';
import { Buffer } from 'react-native-nitro-buffer';
import { AppState, AppStateStatus } from 'react-native';
import type { HttpServer as NitroHttpServer, HttpRequest, HttpResponse } from './HttpServer.nitro';
import { LoopRef } from './loopRef';
import { awaitPendingNativeStop, stopWithTracking, trackNativeStop } from './nativeStopQueue';
import { installRequestAbortedHandler, trackRequest, untrackRequest } from './requestAbort';

// ========== Types ==========

type RequestListener = (req: IncomingMessage, res: ServerResponse) => void;

interface ServerOptions {
    IncomingMessage?: typeof IncomingMessage;
    ServerResponse?: typeof ServerResponse;
    requestTimeout?: number;
    headersTimeout?: number;
    keepAliveTimeout?: number;
    /**
     * Automatically detect and restart the server after iOS background suspension.
     * Defaults to false.
     */
    autoRestart?: boolean;
}

interface AddressInfo {
    address: string;
    family: string;
    port: number;
}

// ========== STATUS_CODES ==========

export const STATUS_CODES: Record<number, string> = {
    100: 'Continue',
    101: 'Switching Protocols',
    102: 'Processing',
    200: 'OK',
    201: 'Created',
    202: 'Accepted',
    203: 'Non-Authoritative Information',
    204: 'No Content',
    205: 'Reset Content',
    206: 'Partial Content',
    300: 'Multiple Choices',
    301: 'Moved Permanently',
    302: 'Found',
    303: 'See Other',
    304: 'Not Modified',
    307: 'Temporary Redirect',
    308: 'Permanent Redirect',
    400: 'Bad Request',
    401: 'Unauthorized',
    402: 'Payment Required',
    403: 'Forbidden',
    404: 'Not Found',
    405: 'Method Not Allowed',
    406: 'Not Acceptable',
    407: 'Proxy Authentication Required',
    408: 'Request Timeout',
    409: 'Conflict',
    410: 'Gone',
    411: 'Length Required',
    412: 'Precondition Failed',
    413: 'Payload Too Large',
    414: 'URI Too Long',
    415: 'Unsupported Media Type',
    416: 'Range Not Satisfiable',
    417: 'Expectation Failed',
    418: "I'm a Teapot",
    422: 'Unprocessable Entity',
    425: 'Too Early',
    426: 'Upgrade Required',
    428: 'Precondition Required',
    429: 'Too Many Requests',
    431: 'Request Header Fields Too Large',
    451: 'Unavailable For Legal Reasons',
    500: 'Internal Server Error',
    501: 'Not Implemented',
    502: 'Bad Gateway',
    503: 'Service Unavailable',
    504: 'Gateway Timeout',
    505: 'HTTP Version Not Supported',
};

// ========== METHODS ==========

export const METHODS: string[] = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS'];

// ========== IncomingMessage ==========

/**
 * Node.js compatible IncomingMessage implementation
 * Implements a subset of http.IncomingMessage interface
 */
export class IncomingMessage extends EventEmitter {
    // Core properties
    readonly method: string;
    readonly url: string;
    readonly headers: Record<string, string | string[] | undefined>;
    readonly rawHeaders: string[];
    readonly httpVersion: string = '1.1';
    readonly httpVersionMajor: number = 1;
    readonly httpVersionMinor: number = 1;

    // Stream state
    readable: boolean = true;
    complete: boolean = false;

    /**
     * True if the client disconnected before the response was sent (mirrors Node's
     * deprecated `IncomingMessage#aborted`, still widely used).
     *
     * Not complementary to `complete`: `complete` only means the request body
     * finished reading and may already be `true` when the client disconnects
     * while the handler is still running.
     */
    aborted: boolean = false;

    /** Whether the underlying connection has been destroyed (Node: `req.destroyed`). */
    destroyed: boolean = false;

    // Internal
    private _requestId: string;
    private _nativeServer: NitroHttpServer;
    private _paused: boolean = false;
    private _reading: boolean = false;
    private _responseEnded: boolean = false;

    constructor(request: HttpRequest, nativeServer: NitroHttpServer) {
        super();
        this._nativeServer = nativeServer;
        this._requestId = request.requestId;
        this.method = request.method;
        this.url = request.path;
        this.headers = this._normalizeHeaders(request.headers);
        this.rawHeaders = this._toRawHeaders(request.headers);
    }

    /**
     * Convert headers to lowercase keys (Node.js convention)
     */
    private _normalizeHeaders(headers: Record<string, string>): Record<string, string | string[] | undefined> {
        const normalized: Record<string, string | string[] | undefined> = {};
        for (const [key, value] of Object.entries(headers)) {
            const lowerKey = key.toLowerCase();
            if (normalized[lowerKey]) {
                // Handle duplicate headers
                const existing = normalized[lowerKey];
                if (Array.isArray(existing)) {
                    existing.push(value);
                } else {
                    normalized[lowerKey] = [existing as string, value];
                }
            } else {
                normalized[lowerKey] = value;
            }
        }
        return normalized;
    }

    /**
     * Convert to rawHeaders format [key1, value1, key2, value2, ...]
     */
    private _toRawHeaders(headers: Record<string, string>): string[] {
        const raw: string[] = [];
        for (const [key, value] of Object.entries(headers)) {
            raw.push(key, value);
        }
        return raw;
    }

    /**
     * Start pushing data when listeners are attached
     */
    _startReading(): void {
        // Use setImmediate to allow event listeners to be attached first
        setTimeout(() => {
            this.resume();
        }, 0);
    }

    // Stream-like methods
    read(_size?: number): Buffer | null {
        // In our push-based simulation, read() just resumes if paused
        if (this._paused) {
            this.resume();
        }
        return null;
    }

    pause(): this {
        this._paused = true;
        return this;
    }

    resume(): this {
        this._paused = false;
        this._pump();
        return this;
    }

    _markResponseEnded(): void {
        this._responseEnded = true;
    }

    private async _pump(): Promise<void> {
        if (this._reading || this._paused || !this.readable || this.aborted || this.destroyed) return;
        this._reading = true;

        try {
            while (!this._paused && this.readable && !this.aborted && !this.destroyed) {
                const chunk = await this._nativeServer.readRequestBodyChunk(this._requestId);

                if (this.aborted || this.destroyed || !this.readable) {
                    break;
                }

                if (!chunk || chunk.length === 0) {
                    this.readable = false;
                    this.complete = true;
                    this.emit('end');
                    break;
                }

                this.emit('data', Buffer.from(chunk, 'utf-8'));
            }
        } catch (err) {
            if (!this.aborted && !this.destroyed && this.readable && !this._responseEnded) {
                this.emit('error', err);
            }
        } finally {
            this._reading = false;
        }
    }

    setEncoding(_encoding: string): this {
        return this;
    }

    destroy(_error?: Error): this {
        this.readable = false;
        this.emit('close');
        return this;
    }

    /**
     * Called when the native layer reports a client disconnect (invoked by `requestAbort.ts`).
     *
     * Event order matches Node 22 (`aborted` -> `error` (ECONNRESET) -> `close`;
     * `res` emits `close` via `ServerResponse._handleAborted`).
     *
     * `'error'` is emitted only when listeners are attached, matching Node's observable
     * behavior (note: `eventemitter3` does not throw on unhandled `'error'` events).
     */
    _handleAborted(): void {
        if (this.aborted) return;
        this.aborted = true;
        this.destroyed = true;
        this.readable = false;
        this.emit('aborted');
        if (this.listenerCount('error') > 0) {
            const err = new Error('aborted') as Error & { code?: string };
            // Matches Node's connResetException('aborted').
            err.code = 'ECONNRESET';
            this.emit('error', err);
        }
        this.emit('close');
    }

    // Pipe support (basic)
    pipe<T extends NodeJS.WritableStream>(destination: T): T {
        this.on('data', (chunk) => destination.write(chunk));
        this.on('end', () => {
            if (typeof (destination as any).end === 'function') {
                (destination as any).end();
            }
        });
        this.resume();
        return destination;
    }
}

// ========== ServerResponse ==========

/**
 * Node.js compatible ServerResponse implementation
 * Implements a subset of http.ServerResponse interface
 */
export class ServerResponse extends EventEmitter {
    // Status
    statusCode: number = 200;
    statusMessage: string = '';

    // Headers
    headersSent: boolean = false;
    private _headers: Map<string, string | number | readonly string[]> = new Map();

    // Internal
    private _requestId: string;
    private _nativeServer: NitroHttpServer;
    private _finished: boolean = false;
    private _resolveNativeRequest: (response: HttpResponse) => void;

    // Writable stream
    writable: boolean = true;
    writableEnded: boolean = false;
    writableFinished: boolean = false;

    /**
     * Whether the response channel has been destroyed (Node: `res.destroyed`).
     *
     * Set to `true` on client disconnect without changing `writableEnded`, matching Node.
     */
    destroyed: boolean = false;

    constructor(
        requestId: string,
        nativeServer: NitroHttpServer,
        resolveNativeRequest: (response: HttpResponse) => void
    ) {
        super();
        this._requestId = requestId;
        this._nativeServer = nativeServer;
        this._resolveNativeRequest = resolveNativeRequest;
    }

    /**
     * Sets a single header value
     */
    setHeader(name: string, value: string | number | readonly string[]): this {
        if (this.headersSent) {
            throw new Error('Cannot set headers after they are sent');
        }
        this._headers.set(name.toLowerCase(), value);
        return this;
    }

    /**
     * Gets a header value
     */
    getHeader(name: string): string | number | readonly string[] | undefined {
        return this._headers.get(name.toLowerCase());
    }

    /**
     * Returns header names
     */
    getHeaderNames(): string[] {
        return Array.from(this._headers.keys());
    }

    /**
     * Returns all headers as object
     */
    getHeaders(): Record<string, string | number | readonly string[] | undefined> {
        const headers: Record<string, string | number | readonly string[] | undefined> = {};
        for (const [key, value] of this._headers) {
            headers[key] = value;
        }
        return headers;
    }

    /**
     * Checks if a header exists
     */
    hasHeader(name: string): boolean {
        return this._headers.has(name.toLowerCase());
    }

    /**
     * Removes a header
     */
    removeHeader(name: string): void {
        if (this.headersSent) {
            throw new Error('Cannot remove headers after they are sent');
        }
        this._headers.delete(name.toLowerCase());
    }

    /**
     * Sends response headers
     */
    writeHead(
        statusCode: number,
        statusMessage?: string | Record<string, string | number | readonly string[]>,
        headers?: Record<string, string | number | readonly string[]>
    ): this {
        if (this.headersSent) {
            throw new Error('Cannot write headers after they are sent');
        }

        this.statusCode = statusCode;

        // Handle overloaded signatures
        if (typeof statusMessage === 'object') {
            headers = statusMessage;
            statusMessage = undefined;
        }

        if (typeof statusMessage === 'string') {
            this.statusMessage = statusMessage;
        }

        if (headers) {
            for (const [key, value] of Object.entries(headers)) {
                this.setHeader(key, value);
            }
        }

        return this;
    }

    /**
     * Writes data to response body
     */
    write(
        chunk: string | Buffer,
        encodingOrCallback?: BufferEncoding | (() => void),
        callback?: () => void
    ): boolean {
        if (this._ended) {
            const err = new Error('write after end');
            this.emit('error', err);
            return false;
        }

        let encoding: BufferEncoding = 'utf-8';
        let cb: (() => void) | undefined;

        if (typeof encodingOrCallback === 'function') {
            cb = encodingOrCallback;
        } else if (typeof encodingOrCallback === 'string') {
            encoding = encodingOrCallback;
            cb = callback;
        }

        let strChunk: string;
        if (Buffer.isBuffer(chunk)) {
            strChunk = chunk.toString(encoding);
        } else {
            strChunk = String(chunk);
        }

        // Mark headers as sent conceptually (though we send them at the end internally)
        this.headersSent = true;

        // Enqueue into the write chain to preserve chunk ordering.
        this._enqueueNativeWrite(
            () => this._nativeServer.writeResponseChunk(this._requestId, strChunk),
            cb
        );

        return true;
    }

    private _ended: boolean = false;

    /**
     * Serializes native writes to preserve chunk ordering.
     *
     * `writeResponseChunk`, `endResponse`, and `sendBinaryResponse` are Nitro
     * `Promise::async` calls executed concurrently on a thread pool. Fire-and-forget
     * calls could reach the Rust chunk accumulator out of order, or let a late
     * `writeResponseChunk` arrive after `cleanup_request_state` and recreate a leaked
     * accumulator entry.
     */
    private _writeChain: Promise<unknown> = Promise.resolve();

    /**
     * Enqueue a native write onto the serial chain.
     *
     * A failed write emits `'error'` and resolves the chain link so subsequent writes
     * still execute in order rather than stalling the response (matching Node).
     */
    private _enqueueNativeWrite<T>(op: () => Promise<T>, onDone?: () => void): void {
        this._writeChain = this._writeChain
            .then(() => op())
            .then(() => { if (onDone) onDone(); })
            .catch((err) => { this.emit('error', err); });
    }

    /**
     * Ends the response
     */
    end(
        chunkOrCallback?: string | Buffer | ArrayBuffer | ArrayBufferView | (() => void),
        encodingOrCallback?: BufferEncoding | (() => void),
        callback?: () => void
    ): this {
        if (this._ended) {
            return this;
        }

        let chunk: string | Buffer | ArrayBuffer | ArrayBufferView | undefined;
        let encoding: BufferEncoding = 'utf-8';
        let cb: (() => void) | undefined;

        // Parse overloaded arguments
        if (typeof chunkOrCallback === 'function') {
            cb = chunkOrCallback;
        } else {
            chunk = chunkOrCallback;
            if (typeof encodingOrCallback === 'function') {
                cb = encodingOrCallback;
            } else if (typeof encodingOrCallback === 'string') {
                encoding = encodingOrCallback;
                cb = callback;
            }
        }

        // Handle binary chunk (ArrayBuffer or View)
        if (chunk && typeof chunk === 'object' &&
            (chunk instanceof ArrayBuffer || ArrayBuffer.isView(chunk))) {

            // Prepare buffer
            const buffer = chunk instanceof ArrayBuffer
                ? chunk
                : (chunk.byteLength === chunk.buffer.byteLength && chunk.byteOffset === 0)
                    ? chunk.buffer
                    : chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.byteLength);

            // Use internal binary response path
            // Collect headers
            const headers: Record<string, string> = {};
            for (const [key, value] of this._headers) {
                if (Array.isArray(value)) {
                    headers[key] = value.join(', ');
                } else {
                    headers[key] = String(value);
                }
            }

            const headersJson = JSON.stringify(headers);

            // Enqueue in the write chain so pending writes finish before sendBinaryResponse
            // cleans up state; native send_response prepends any accumulated chunks.
            this._enqueueNativeWrite(
                () => this._nativeServer.sendBinaryResponse(
                    this._requestId, this.statusCode, headersJson, buffer as ArrayBuffer
                ),
                () => {
                    this._finished = true;
                    this.writableFinished = true;
                    this.emit('finish');

                    // Resolve the native request promise
                    this._resolveNativeRequest({
                        statusCode: this.statusCode,
                        headers: headers,
                        body: ''
                    });

                    if (cb) cb();
                }
            );

            this._ended = true;
            this.writableEnded = true;
            this.headersSent = true;
            this.writable = false;
            return this;
        }

        // Write final chunk if provided (String legacy path)
        if (chunk !== undefined) {
            // Enqueue directly on the write chain so the final chunk lands before endResponse.
            let strChunk = Buffer.isBuffer(chunk) ? chunk.toString(encoding) : String(chunk);
            this._enqueueNativeWrite(
                () => this._nativeServer.writeResponseChunk(this._requestId, strChunk),
                () => this._finalizeResponse(cb)
            );
        } else {
            // Even without a final chunk, endResponse must wait for all pending writes.
            this._enqueueNativeWrite(
                () => Promise.resolve(),
                () => this._finalizeResponse(cb)
            );
        }

        this._ended = true;
        this.writableEnded = true;
        this.headersSent = true;
        this.writable = false;

        return this;
    }

    private _finalizeResponse(cb?: () => void) {
        // Collect headers
        const headers: Record<string, string> = {};
        for (const [key, value] of this._headers) {
            if (Array.isArray(value)) {
                headers[key] = value.join(', ');
            } else {
                headers[key] = String(value);
            }
        }

        const headersJson = JSON.stringify(headers);

        this._nativeServer.endResponse(this._requestId, this.statusCode, headersJson)
            .then(() => {
                this._finished = true;
                this.writableFinished = true;
                this.emit('finish');

                // Resolve the native request promise to prevent timeout/hangs in the bridge if it's waiting
                // We send a dummy response because we handled it via streaming
                this._resolveNativeRequest({
                    statusCode: this.statusCode,
                    headers: headers,
                    body: '' // Body handled via streaming
                });

                if (cb) cb();
            })
            .catch((err) => {
                this.emit('error', err);
            });
    }

    /**
     * Flushes headers (no-op in our implementation)
     */
    flushHeaders(): void {
        this.headersSent = true;
    }

    /**
     * Cork/Uncork (no-op in our implementation)
     */
    cork(): void { }
    uncork(): void { }

    get finished(): boolean {
        return this._finished;
    }

    /**
     * Called when the native layer reports a client disconnect (invoked by `requestAbort.ts`).
     *
     * Intentionally leaves `writableEnded` and `finished` unchanged, and keeps
     * post-disconnect `write`/`end` silent, matching Node's behavior.
     */
    _handleAborted(): void {
        if (this.destroyed) return;
        this.destroyed = true;
        this.writable = false;
        this.emit('close');
    }


}

// ========== Server ==========

/**
 * Node.js compatible Server implementation
 * Implements a subset of http.Server interface
 */
export class Server extends EventEmitter {
    /**
     * Keeps the event loop alive while the server is running or starting.
     * Native `start()` is asynchronous (`'listening'` emits only after it resolves),
     * so without a ref a headless JS loop would exit before `'listening'` fires.
     */
    private _loopRef = new LoopRef('http-server.Server');

    // Server state
    listening: boolean = false;
    private _port: number = 0;
    private _host: string = '127.0.0.1';
    private _requestListener?: RequestListener;
    private _intentionallyStopped = false;

    // Native server
    private _nativeServer: NitroHttpServer;

    // Options
    private _options: ServerOptions;

    // AppState listener for autoRestart
    private _appStateSub: { remove: () => void } | null = null;

    constructor(options?: ServerOptions | RequestListener, requestListener?: RequestListener) {
        super();

        // Handle overloaded constructor
        if (typeof options === 'function') {
            this._requestListener = options;
            this._options = {};
        } else {
            this._options = options || {};
            this._requestListener = requestListener;
        }

        // Get the native server instance
        this._nativeServer = NitroModules.createHybridObject<NitroHttpServer>('HttpServer');
    }

    /**
     * Starts listening for connections
     */
    listen(port: number, hostname?: string | (() => void), backlog?: number | (() => void), callback?: () => void): this;
    listen(port: number, callback?: () => void): this;
    listen(
        port: number,
        hostnameOrCallback?: string | (() => void),
        backlogOrCallback?: number | (() => void),
        callback?: () => void
    ): this {
        if (this.listening) {
            throw new Error('Server is already listening');
        }

        this._port = port;
        this._intentionallyStopped = false;

        // Parse hostname and callback from overloaded arguments
        let hostname: string | undefined;
        let cb: (() => void) | undefined;
        if (typeof hostnameOrCallback === 'string') {
            hostname = hostnameOrCallback;
            if (typeof backlogOrCallback === 'function') {
                cb = backlogOrCallback;
            } else {
                cb = callback;
            }
        } else if (typeof hostnameOrCallback === 'function') {
            cb = hostnameOrCallback;
        } else if (typeof backlogOrCallback === 'function') {
            cb = backlogOrCallback;
        } else {
            cb = callback;
        }

        // Set the host
        if (hostname) {
            this._host = hostname;
        }

        // Acquire loop ref before the async native start so the loop stays alive until 'listening'.
        this._loopRef.acquire();
        // Install the global client-disconnect handler (idempotent; safe across auto-restarts).
        installRequestAbortedHandler(this._nativeServer);
        // Wait for any in-flight native stop before starting (singleton native server; see nativeStopQueue.ts).
        awaitPendingNativeStop()
            .then(() => this._nativeServer.start(port, this._handleNativeRequest.bind(this), hostname))
            .then((actualPort) => {
                if (actualPort > 0) {
                    this._port = actualPort;
                    this.listening = true;

                    if (this._options.autoRestart) {
                        this._registerAutoRestart();
                    }

                    this.emit('listening');
                    if (cb) cb();
                } else {
                    // Release the loop ref if start failed.
                    this._loopRef.release();
                    const err = new Error(`Failed to start server on port ${port}`);
                    this.emit('error', err);
                }
            })
            .catch((err) => {
                this._loopRef.release();
                this.emit('error', err);
            });

        return this;
    }

    /**
     * Stops the server
     */
    close(callback?: (err?: Error) => void): this {
        this._intentionallyStopped = true;
        this._unregisterAutoRestart();

        if (!this.listening) {
            if (callback) {
                setTimeout(() => callback(new Error('Server is not running')), 0);
            }
            return this;
        }

        const stopping = this._nativeServer.stop();
        // Track the native stop so subsequent start calls wait for teardown.
        trackNativeStop(stopping);
        stopping
            .then(() => {
                // Release the loop ref before emitting 'close' (listeners may start a new
                // server or check active handles). Keep the ref held if stop() fails.
                this._loopRef.release();
                this.listening = false;
                this.emit('close');
                if (callback) callback();
            })
            .catch((err) => {
                if (callback) callback(err);
            });

        return this;
    }

    /**
     * Registers an AppState listener to auto-restart the server
     * after iOS background suspension or Android process recovery.
     */
    private async _probeAlive(): Promise<boolean> {
        // Probe this instance's own port independently.
        // Use a 500ms timeout and retry once after 100ms: the JS thread may be busy when
        // returning to the foreground, and too short a timeout would falsely restart a healthy server.
        const probe = async (): Promise<boolean> => {
            const controller = new AbortController();
            const timeout = setTimeout(() => controller.abort(), 500);
            try {
                await fetch(`http://127.0.0.1:${this._port}/`, {
                    method: 'HEAD',
                    signal: controller.signal,
                });
                return true;
            } catch {
                return false;
            } finally {
                // Clear the timer so immediate connection-refused rejections do not leave dangling timers.
                clearTimeout(timeout);
            }
        };
        if (await probe()) return true;
        await new Promise(r => setTimeout(r, 100));
        return probe();
    }

    private _registerAutoRestart(): void {
        if (this._appStateSub) return;

        this._appStateSub = AppState.addEventListener('change', async (state: AppStateStatus) => {
            if (state !== 'active') return;
            if (this._intentionallyStopped) return;

            const alive = await this._probeAlive();
            if (!alive) {
                console.log('[http.Server] Detected server is dead, auto-restarting...');
                this.listening = false;
                // Bypasses close(), so release the loop ref if native stop succeeds before re-acquiring.
                try { await stopWithTracking(this._nativeServer.stop()); this._loopRef.release(); } catch (_) { /* ignore */ }

                try {
                    this._loopRef.acquire();
                    const actualPort = await this._nativeServer.start(
                        this._port,
                        this._handleNativeRequest.bind(this),
                        this._host
                    );
                    if (actualPort > 0) {
                        this._port = actualPort;
                        this.listening = true;
                        console.log(`[http.Server] Auto-restarted on port ${actualPort}`);
                    } else {
                        this._loopRef.release();
                    }
                } catch (e) {
                    this._loopRef.release();
                    console.error('[http.Server] Auto-restart failed:', e);
                }
            }
        });
    }

    private _unregisterAutoRestart(): void {
        if (this._appStateSub) {
            this._appStateSub.remove();
            this._appStateSub = null;
        }
    }

    /**
     * Returns the bound address
     */
    address(): AddressInfo | string | null {
        if (!this.listening) return null;
        return {
            address: this._host,
            family: 'IPv4',
            port: this._port,
        };
    }

    /**
     * Handle incoming request from native layer
     */
    private _handleNativeRequest(request: HttpRequest): HttpResponse | Promise<HttpResponse> {
        return new Promise((resolve) => {
            // Create Node.js compatible request/response objects
            // Pass the native server instance for streaming data access
            const req = new IncomingMessage(request, this._nativeServer);
            const res = new ServerResponse(request.requestId, this._nativeServer, (response) => {
                // Untrack once the response is resolved.
                req._markResponseEnded();
                untrackRequest(request.requestId);
                resolve(response);
            });

            // Register abort tracking before emitting 'request' so disconnects during any
            // async work in the listener are caught immediately.
            const abortEntry = trackRequest(request.requestId);
            abortEntry.listeners.add(() => {
                req._handleAborted();
                res._handleAborted();
            });

            // Emit request event
            this.emit('request', req, res);

            // Trigger body reading
            req._startReading();

            // Call request listener if provided
            if (this._requestListener) {
                try {
                    this._requestListener(req, res);
                } catch (err) {
                    // Handle synchronous errors
                    if (!res.headersSent) {
                        res.statusCode = 500;
                        res.end('Internal Server Error');
                    }
                    this.emit('error', err);
                }
            }
        });
    }

    // Additional server methods (timeouts - no-op in our implementation)
    setTimeout(_msecs?: number, _callback?: () => void): this {
        return this;
    }

    get timeout(): number {
        return 0;
    }

    set timeout(_value: number) { }

    get headersTimeout(): number {
        return this._options.headersTimeout || 60000;
    }

    set headersTimeout(value: number) {
        this._options.headersTimeout = value;
    }

    /**
     * Compatibility placeholder only (currently has no effect).
     *
     * To configure the JS handler response timeout, use `request_timeout_secs`
     * (in seconds, default 30) on `createConfigServer`:
     *
     *   createConfigServer(port, handler, { request_timeout_secs: 600 })
     *
     * `createServer()` uses the plain server path and does not accept config.
     */
    get requestTimeout(): number {
        return this._options.requestTimeout || 300000;
    }

    set requestTimeout(value: number) {
        this._options.requestTimeout = value;
    }

    get keepAliveTimeout(): number {
        return this._options.keepAliveTimeout || 5000;
    }

    set keepAliveTimeout(value: number) {
        this._options.keepAliveTimeout = value;
    }
}

// ========== createServer ==========

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
export function createServer(requestListener?: RequestListener): Server;
export function createServer(options: ServerOptions, requestListener?: RequestListener): Server;
export function createServer(
    optionsOrListener?: ServerOptions | RequestListener,
    requestListener?: RequestListener
): Server {
    return new Server(optionsOrListener, requestListener);
}

// Export types for external use
export type { RequestListener, ServerOptions, AddressInfo };