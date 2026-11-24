import { type HybridObject } from 'react-native-nitro-modules'

export interface HttpRequest {
    requestId: string
    method: string
    path: string
    headers: Record<string, string>
    body?: string
    binaryBody?: ArrayBuffer
}

export interface HttpResponse {
    statusCode: number
    headers?: Record<string, string>
    body?: string

}

export interface ServerStats {
    totalRequests: number
    activeConnections: number
    bytesSent: number
    bytesReceived: number
    uptime: number
    errorCount: number
}

interface BaseMount {
    path: string
    /** Custom response headers (no-op for rewrite, websocket, and upload mounts). */
    headers?: Record<string, string>
}

export interface WebDavMount extends BaseMount {
    type: 'webdav'
    root: string
}

export interface ZipMount extends BaseMount {
    type: 'zip'
    zip_file: string
}

export interface DirListConfig {
    enabled: boolean
    show_hidden?: boolean
}

export interface StaticMount extends BaseMount {
    type: 'static'
    root: string
    dir_list?: DirListConfig
    default_index?: string[]
}

export interface UploadMount extends BaseMount {
    type: 'upload'
    temp_dir: string
}

/** Upload mount that passes file contents to JS as an ArrayBuffer (up to 100MB). */
export interface BufferUploadMount extends BaseMount {
    type: 'buffer_upload'
}

export interface RewriteRule {
    /** Regular expression pattern. */
    pattern: string
    /** Replacement target (supports capture groups such as $1, $2). */
    replacement: string
}

export interface RewriteMount {
    type: 'rewrite'
    rules: RewriteRule[]
}

export interface WebSocketMount extends BaseMount {
    type: 'websocket'
    /** Maximum message size in bytes (default: 64MB). */
    max_message_size?: number
}

export type Mountable = WebDavMount | ZipMount | StaticMount | UploadMount | BufferUploadMount | RewriteMount | WebSocketMount

export interface CorsConfig {
    /** Allowed origin (default: "*"). */
    origin?: string
    /** Allowed HTTP methods (defaults to common HTTP + WebDAV methods). */
    methods?: string[]
    /** Allowed headers (default: ["*"]; echoes Access-Control-Request-Headers on preflight). */
    headers?: string[]
    /** Allow credentials (default: false; echoes the request Origin when true). */
    credentials?: boolean
    /** Preflight cache duration in seconds. */
    max_age?: number
}

export interface ServerConfig {
    /** Static file root directory (optional, used as the default static mount). */
    root_dir?: string
    verbose?: boolean | 'off' | 'error' | 'warn' | 'info' | 'debug'
    mime_types?: Record<string, string>
    mounts?: Mountable[]
    /** CORS configuration: true enables defaults (Allow-Origin: *), object customizes, omitted disables. */
    cors?: boolean | CorsConfig
    /**
     * Maximum time in seconds to wait for the JS handler to return a response (default: 30).
     * Increase for long-polling or SSE; otherwise connections are terminated with 500.
     *
     * Global state on the singleton server: only updated when explicitly provided, and
     * re-applied when restarting a config server.
     */
    request_timeout_secs?: number
    /**
     * Maximum request body size in bytes for the JS callback path (default: 100MB).
     *
     * Requests exceeding this limit are rejected with 413 before invoking the JS handler.
     * Plugin mounts (static, zip, upload, webdav, etc.) do not buffer the body here and
     * are not subject to this limit.
     *
     * Global state on the singleton server: only updated when explicitly provided, and
     * re-applied when restarting a config server.
     */
    max_body_size?: number
}

export type WebSocketEventType = 'open' | 'message' | 'close' | 'error'

export interface WebSocketEvent {
    connectionId: string
    type: WebSocketEventType
    path?: string
    /** Query string (open event only). */
    query?: string
    /** JSON-encoded HTTP headers (open event only). */
    headersJson?: string
    textData?: string
    binaryData?: ArrayBuffer
    closeCode?: number
    closeReason?: string
    errorMessage?: string
}

export type WebSocketHandler = (event: WebSocketEvent) => void

export type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse

export interface HttpServer extends HybridObject<{
    ios: 'swift',
    android: 'kotlin'
}> {
    /**
     * Start the HTTP server.
     * @param port Port number to bind.
     * @param handler Request handler callback.
     * @param host IP address to listen on (default: 127.0.0.1).
     * @param workerThreads Tokio worker thread count. Omit or 0 to use CPU core count (tokio default).
     *   The runtime is created on start() and destroyed on stop(), so this can vary per start.
     *   Fewer threads reduce concurrency under slow awaits (DB/network), though on high-core devices
     *   reducing worker count can lower mutex contention and per-request CPU overhead.
     * @returns Actual bound port (0 indicates failure).
     */
    start(port: number, handler: RequestHandler, host?: string, workerThreads?: number): Promise<number>

    /**
     * Send an HTTP response.
     * @param requestId Request identifier.
     * @param response HTTP response payload.
     * @returns Whether the response was sent successfully.
     */
    sendResponse(requestId: string, response: HttpResponse): Promise<boolean>

    /**
     * Stop the HTTP server.
     */
    stop(): Promise<void>

    /**
     * Retrieve server statistics.
     */
    getStats(): Promise<ServerStats>

    /**
     * Check whether the server is currently running.
     */
    isRunning(): Promise<boolean>

    /**
     * Start the static file server.
     * @param port Port number to bind.
     * @param rootDir Static file root directory path.
     * @param host IP address to listen on (default: 127.0.0.1).
     * @returns Actual bound port (0 indicates failure).
     */
    startStaticServer(port: number, rootDir: string, host?: string): Promise<number>

    /**
     * Stop the static file server.
     */
    stopStaticServer(): Promise<void>

    /**
     * Read the next chunk of the request body.
     * @param requestId Request identifier.
     * @returns Body chunk string (empty string indicates EOF).
     */
    readRequestBodyChunk(requestId: string): Promise<string>

    /**
     * Write a chunk of the response body.
     * @param requestId Request identifier.
     * @param chunk Response body chunk.
     * @returns Whether the write succeeded.
     */
    writeResponseChunk(requestId: string, chunk: string): Promise<boolean>

    /**
     * Finalize a streamed response.
     * @param requestId Request identifier.
     * @param statusCode HTTP status code.
     * @param headersJson JSON-encoded response headers.
     * @returns Whether finalization succeeded.
     */
    endResponse(requestId: string, statusCode: number, headersJson: string): Promise<boolean>

    /**
     * Set global CORS configuration (applies to all server types; call before starting).
     * @param corsJson JSON string: "true" for defaults, a serialized CorsConfig object, or "false" to disable.
     */
    setCorsConfig(corsJson: string): void

    /**
     * Send a binary response body (synchronously copies the ArrayBuffer on the JS thread
     * to avoid cross-thread lifetime issues).
     * @param requestId Request identifier.
     * @param statusCode HTTP status code.
     * @param headersJson JSON-encoded response headers.
     * @param body Binary response body.
     * @returns Whether the response was sent successfully.
     */
    sendBinaryResponse(requestId: string, statusCode: number, headersJson: string, body: ArrayBuffer): Promise<boolean>

    /**
     * Start the App HTTP server (hybrid static files and callback handler).
     * @param port Port number to bind.
     * @param rootDir Static file root directory path.
     * @param handler Request handler callback.
     * @param host IP address to listen on (default: 127.0.0.1).
     * @returns Actual bound port (0 indicates failure).
     */
    startAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<number>

    /**
     * Stop the App HTTP server.
     */
    stopAppServer(): Promise<void>

    /**
     * Start a configurable App HTTP server with plugin mounts (e.g. WebDAV, Zip, WebSocket).
     * @param port Port number to bind.
     * @param handler Request handler callback.
     * @param configJson JSON-encoded ServerConfig.
     * @param host IP address to listen on (default: 127.0.0.1).
     * @returns Actual bound port (0 indicates failure).
     */
    startServerWithConfig(port: number, handler: RequestHandler, configJson: string, host?: string): Promise<number>

    // ==================== Request Abort Notification ====================

    /**
     * Register the callback invoked when a client aborts a request before a response is sent.
     *
     * Global single callback (last write wins), matching `setWebSocketHandler`.
     * Fires only when no response was sent (not on normal completion, timeout, or handler throw).
     *
     * @param handler Callback receiving the aborted `requestId` on the JS thread.
     */
    setRequestAbortedHandler(handler: (requestId: string) => void): void

    // ==================== WebSocket API ====================

    /**
     * Register the global WebSocket event handler.
     */
    setWebSocketHandler(handler: WebSocketHandler): void

    /**
     * Send a WebSocket text message.
     */
    wsSendText(connectionId: string, message: string): Promise<boolean>

    /**
     * Send a WebSocket binary message.
     */
    wsSendBinary(connectionId: string, data: ArrayBuffer): Promise<boolean>

    /**
     * Close a WebSocket connection.
     * @param connectionId Connection identifier.
     * @param code Close status code (default: 1000).
     * @param reason Close reason string.
     */
    wsClose(connectionId: string, code?: number, reason?: string): Promise<boolean>
}