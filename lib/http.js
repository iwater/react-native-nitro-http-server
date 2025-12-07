"use strict";
// src/http.ts
// Node.js Compatible HTTP Interface for React Native
Object.defineProperty(exports, "__esModule", { value: true });
exports.Server = exports.ServerResponse = exports.IncomingMessage = exports.METHODS = exports.STATUS_CODES = void 0;
exports.createServer = createServer;
const events_1 = require("events");
const react_native_nitro_modules_1 = require("react-native-nitro-modules");
// ========== STATUS_CODES ==========
exports.STATUS_CODES = {
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
exports.METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS'];
// ========== IncomingMessage ==========
/**
 * Node.js compatible IncomingMessage implementation
 * Implements a subset of http.IncomingMessage interface
 */
class IncomingMessage extends events_1.EventEmitter {
    constructor(request) {
        super();
        this.httpVersion = '1.1';
        this.httpVersionMajor = 1;
        this.httpVersionMinor = 1;
        // Stream state
        this.readable = true;
        this.complete = false;
        this._bodyEmitted = false;
        this._requestId = request.requestId;
        this.method = request.method;
        this.url = request.path;
        this.headers = this._normalizeHeaders(request.headers);
        this.rawHeaders = this._toRawHeaders(request.headers);
        this._body = request.body || '';
    }
    /**
     * Convert headers to lowercase keys (Node.js convention)
     */
    _normalizeHeaders(headers) {
        const normalized = {};
        for (const [key, value] of Object.entries(headers)) {
            const lowerKey = key.toLowerCase();
            if (normalized[lowerKey]) {
                // Handle duplicate headers
                const existing = normalized[lowerKey];
                if (Array.isArray(existing)) {
                    existing.push(value);
                }
                else {
                    normalized[lowerKey] = [existing, value];
                }
            }
            else {
                normalized[lowerKey] = value;
            }
        }
        return normalized;
    }
    /**
     * Convert to rawHeaders format [key1, value1, key2, value2, ...]
     */
    _toRawHeaders(headers) {
        const raw = [];
        for (const [key, value] of Object.entries(headers)) {
            raw.push(key, value);
        }
        return raw;
    }
    /**
     * Emit body data (called internally after listeners are attached)
     */
    _emitBody() {
        if (this._bodyEmitted)
            return;
        this._bodyEmitted = true;
        // Use setImmediate/setTimeout to allow event listeners to be attached
        setTimeout(() => {
            if (this._body) {
                this.emit('data', Buffer.from(this._body, 'utf-8'));
            }
            this.readable = false;
            this.complete = true;
            this.emit('end');
        }, 0);
    }
    // Stream-like methods (minimal implementation)
    read(_size) {
        if (this._bodyEmitted || !this._body)
            return null;
        this._bodyEmitted = true;
        return Buffer.from(this._body, 'utf-8');
    }
    pause() {
        return this;
    }
    resume() {
        this._emitBody();
        return this;
    }
    setEncoding(_encoding) {
        return this;
    }
    destroy(_error) {
        this.readable = false;
        this.emit('close');
        return this;
    }
    // Pipe support (basic)
    pipe(destination) {
        this.on('data', (chunk) => destination.write(chunk));
        this.on('end', () => {
            if (typeof destination.end === 'function') {
                destination.end();
            }
        });
        this._emitBody();
        return destination;
    }
}
exports.IncomingMessage = IncomingMessage;
// ========== ServerResponse ==========
/**
 * Node.js compatible ServerResponse implementation
 * Implements a subset of http.ServerResponse interface
 */
class ServerResponse extends events_1.EventEmitter {
    constructor(requestId, sendResponse) {
        super();
        // Status
        this.statusCode = 200;
        this.statusMessage = '';
        // Headers
        this.headersSent = false;
        this._headers = new Map();
        // Body
        this._body = [];
        this._ended = false;
        this._finished = false;
        // Writable stream
        this.writable = true;
        this.writableEnded = false;
        this.writableFinished = false;
        this._requestId = requestId;
        this._sendResponse = sendResponse;
    }
    /**
     * Sets a single header value
     */
    setHeader(name, value) {
        if (this.headersSent) {
            throw new Error('Cannot set headers after they are sent');
        }
        this._headers.set(name.toLowerCase(), value);
        return this;
    }
    /**
     * Gets a header value
     */
    getHeader(name) {
        return this._headers.get(name.toLowerCase());
    }
    /**
     * Returns header names
     */
    getHeaderNames() {
        return Array.from(this._headers.keys());
    }
    /**
     * Returns all headers as object
     */
    getHeaders() {
        const headers = {};
        for (const [key, value] of this._headers) {
            headers[key] = value;
        }
        return headers;
    }
    /**
     * Checks if a header exists
     */
    hasHeader(name) {
        return this._headers.has(name.toLowerCase());
    }
    /**
     * Removes a header
     */
    removeHeader(name) {
        if (this.headersSent) {
            throw new Error('Cannot remove headers after they are sent');
        }
        this._headers.delete(name.toLowerCase());
    }
    /**
     * Sends response headers
     */
    writeHead(statusCode, statusMessage, headers) {
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
    write(chunk, encodingOrCallback, callback) {
        if (this._ended) {
            const err = new Error('write after end');
            this.emit('error', err);
            return false;
        }
        let encoding = 'utf-8';
        let cb;
        if (typeof encodingOrCallback === 'function') {
            cb = encodingOrCallback;
        }
        else if (typeof encodingOrCallback === 'string') {
            encoding = encodingOrCallback;
            cb = callback;
        }
        let buffer;
        if (Buffer.isBuffer(chunk)) {
            buffer = chunk;
        }
        else {
            buffer = Buffer.from(chunk, encoding);
        }
        this._body.push(buffer);
        this.headersSent = true;
        if (cb) {
            // Simulate async callback
            setTimeout(cb, 0);
        }
        return true;
    }
    /**
     * Ends the response
     */
    end(chunkOrCallback, encodingOrCallback, callback) {
        if (this._ended) {
            return this;
        }
        let chunk;
        let encoding = 'utf-8';
        let cb;
        // Parse overloaded arguments
        if (typeof chunkOrCallback === 'function') {
            cb = chunkOrCallback;
        }
        else {
            chunk = chunkOrCallback;
            if (typeof encodingOrCallback === 'function') {
                cb = encodingOrCallback;
            }
            else if (typeof encodingOrCallback === 'string') {
                encoding = encodingOrCallback;
                cb = callback;
            }
        }
        // Write final chunk if provided
        if (chunk !== undefined) {
            this.write(chunk, encoding);
        }
        this._ended = true;
        this.writableEnded = true;
        this.headersSent = true;
        this.writable = false;
        // Send response via native layer
        this._sendNativeResponse().then(() => {
            this._finished = true;
            this.writableFinished = true;
            this.emit('finish');
            if (cb)
                cb();
        }).catch((err) => {
            this.emit('error', err);
        });
        return this;
    }
    /**
     * Flushes headers (no-op in our implementation)
     */
    flushHeaders() {
        this.headersSent = true;
    }
    /**
     * Cork/Uncork (no-op in our implementation)
     */
    cork() { }
    uncork() { }
    /**
     * Constructs and sends the response via native layer
     */
    async _sendNativeResponse() {
        // Combine body chunks
        const body = Buffer.concat(this._body).toString('utf-8');
        // Convert headers to Record<string, string>
        const headers = {};
        for (const [key, value] of this._headers) {
            if (Array.isArray(value)) {
                headers[key] = value.join(', ');
            }
            else {
                headers[key] = String(value);
            }
        }
        // Construct response object
        const response = {
            statusCode: this.statusCode,
            headers,
            body,
        };
        await this._sendResponse(response);
    }
    get finished() {
        return this._finished;
    }
}
exports.ServerResponse = ServerResponse;
// ========== Server ==========
/**
 * Node.js compatible Server implementation
 * Implements a subset of http.Server interface
 */
class Server extends events_1.EventEmitter {
    constructor(options, requestListener) {
        super();
        // Server state
        this.listening = false;
        this._port = 0;
        // Handle overloaded constructor
        if (typeof options === 'function') {
            this._requestListener = options;
            this._options = {};
        }
        else {
            this._options = options || {};
            this._requestListener = requestListener;
        }
        // Get the native server instance
        this._nativeServer = react_native_nitro_modules_1.NitroModules.createHybridObject('HttpServer');
    }
    listen(port, hostnameOrCallback, backlogOrCallback, callback) {
        if (this.listening) {
            throw new Error('Server is already listening');
        }
        this._port = port;
        // Parse callback from overloaded arguments
        let cb;
        if (typeof hostnameOrCallback === 'function') {
            cb = hostnameOrCallback;
        }
        else if (typeof backlogOrCallback === 'function') {
            cb = backlogOrCallback;
        }
        else {
            cb = callback;
        }
        // Start the native server with our request handler
        this._nativeServer.start(port, this._handleNativeRequest.bind(this))
            .then((success) => {
            if (success) {
                this.listening = true;
                this.emit('listening');
                if (cb)
                    cb();
            }
            else {
                const err = new Error(`Failed to start server on port ${port}`);
                this.emit('error', err);
            }
        })
            .catch((err) => {
            this.emit('error', err);
        });
        return this;
    }
    /**
     * Stops the server
     */
    close(callback) {
        if (!this.listening) {
            if (callback) {
                setTimeout(() => callback(new Error('Server is not running')), 0);
            }
            return this;
        }
        this._nativeServer.stop()
            .then(() => {
            this.listening = false;
            this.emit('close');
            if (callback)
                callback();
        })
            .catch((err) => {
            if (callback)
                callback(err);
        });
        return this;
    }
    /**
     * Returns the bound address
     */
    address() {
        if (!this.listening)
            return null;
        return {
            address: '0.0.0.0',
            family: 'IPv4',
            port: this._port,
        };
    }
    /**
     * Handle incoming request from native layer
     */
    _handleNativeRequest(request) {
        return new Promise((resolve) => {
            // Create Node.js compatible request/response objects
            const req = new IncomingMessage(request);
            const res = new ServerResponse(request.requestId, async (response) => {
                resolve(response);
                return true;
            });
            // Emit request event
            this.emit('request', req, res);
            // Call request listener if provided
            if (this._requestListener) {
                try {
                    this._requestListener(req, res);
                    // Trigger body emission after handler is set up
                    req._emitBody();
                }
                catch (err) {
                    // Handle synchronous errors
                    if (!res.headersSent) {
                        res.statusCode = 500;
                        res.end('Internal Server Error');
                    }
                    this.emit('error', err);
                }
            }
            else {
                // No listener, trigger body emission anyway
                req._emitBody();
            }
        });
    }
    // Additional server methods (timeouts - no-op in our implementation)
    setTimeout(_msecs, _callback) {
        return this;
    }
    get timeout() {
        return 0;
    }
    set timeout(_value) { }
    get headersTimeout() {
        return this._options.headersTimeout || 60000;
    }
    set headersTimeout(value) {
        this._options.headersTimeout = value;
    }
    get requestTimeout() {
        return this._options.requestTimeout || 300000;
    }
    set requestTimeout(value) {
        this._options.requestTimeout = value;
    }
    get keepAliveTimeout() {
        return this._options.keepAliveTimeout || 5000;
    }
    set keepAliveTimeout(value) {
        this._options.keepAliveTimeout = value;
    }
}
exports.Server = Server;
function createServer(optionsOrListener, requestListener) {
    return new Server(optionsOrListener, requestListener);
}
