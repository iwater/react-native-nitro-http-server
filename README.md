# React Native HTTP Server

A high-performance React Native HTTP server library, implemented in Rust, supporting dynamic request handling and static file serving.

[![license](https://img.shields.io/badge/license-ISC-blue.svg)](https://github.com/iwater/rn-http-server/blob/main/LICENSE)
[![platform](https://img.shields.io/badge/platform-ios%20%7C%20android-lightgrey.svg)]()
[中文文档](./README_zh.md)

## ✨ Features

- 🚀 **High Performance**: Built on Rust's Actix-web framework, delivering exceptional performance.
- 📱 **Cross-Platform**: Supports iOS and Android.
- 🔄 **Asynchronous**: Uses Nitro Modules to provide native async APIs.
- 📁 **Static File Serving**: Built-in static file server support.
- 📂 **Directory Listing**: Automatically generate directory listing pages.
- 🎯 **Easy to Use**: TypeScript-friendly API design.
- ⚡ **Zero Copy**: Direct FFI calls to Rust code.
- 🔌 **Plugin System**: Support for WebDAV, Zip mounting, and extensible plugins.
- 🌊 **Streaming APIs**: Support for streaming request/response bodies.
- 📤 **File Upload Plugin**: Support for handling `multipart/form-data` file uploads efficiently (save to disk).
- 💾 **Buffer Upload Plugin**: Handle file uploads in memory with direct `ArrayBuffer` access.
- 🔀 **URL Rewrite Plugin**: Support pattern-based URL rewriting using regular expressions.
- 🔌 **WebSocket Plugin**: Real-time bidirectional communication with full handshake info access.
- ⛔ **Client-disconnect awareness**: `request.signal` / `req.aborted` / `res.destroyed` so you can cancel downstream work.
- 🔄 **Node.js Compatible**: Compatible with Node.js `http` module API.

## 📦 Installation

```bash
npm install react-native-nitro-http-server
# or
yarn add react-native-nitro-http-server
```

### iOS Configuration

Run pod install:

```bash
cd ios && pod install
```

### Android Configuration

No extra configuration needed, autolinking is supported.

## 🚀 Quick Start

### Basic HTTP Server

```typescript
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();

// Start server (returns the actual port, e.g., 8080)
const actualPort = await server.start(8080, async (request) => {
  console.log(`Received request: ${request.method} ${request.path}`);
  
  return {
    statusCode: 200,
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      message: 'Hello from React Native!',
      path: request.path,
    }),
  };
});

console.log(`Server running at http://localhost:${actualPort}`);

// Pass 0 to automatically allocate a random free port
const randomPort = await server.start(0, handler);
console.log(`Server started on random port: ${randomPort}`);

// Get current port
console.log('Current port:', server.port);

// Stop server
// await server.stop();
```

### Binary Response Example

```typescript
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();

await server.start(8080, async (request) => {
  // Return a binary image
  const imageBuffer = new ArrayBuffer(1024); // Your binary data
  
  return {
    statusCode: 200,
    headers: {
      'Content-Type': 'image/png',
    },
    body: imageBuffer, // direct ArrayBuffer support
  };
});
```

### Static File Server

```typescript
import { StaticServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new StaticServer();

// Start static file server
const staticDir = RNFS.DocumentDirectoryPath + '/www';
await server.start(8080, staticDir);

console.log(`Static file server running at http://localhost:8080`);
console.log(`Serving directory: ${staticDir}`);

// Stop static server
// await server.stop();
```

### App Server (Hybrid Mode)

Supports both static file serving and dynamic API handling. It prioritizes serving static files; if the file does not exist, it invokes the callback function.

```typescript
import { AppServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new AppServer();
const staticDir = RNFS.DocumentDirectoryPath + '/www';

// Start app server (hybrid mode)
await server.start(8080, staticDir, async (request) => {
  // This callback is executed when the static file is not found
  return {
    statusCode: 200,
    body: `Dynamic response for ${request.path}`,
  };
});
```

### Config Server (With Plugins)

Supports advanced features like WebDAV and Zip file mounting through plugin configuration.

```typescript
import { createConfigServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const staticDir = RNFS.DocumentDirectoryPath + '/www';

// Configure plugins
const config = {
  root_dir: staticDir,           // Static file root (Optional)
  verbose: 'info',               // Log level: 'off' | 'error' | 'warn' | 'info' | 'debug' (default: 'off')
  mounts: [
    {
      type: 'webdav',
      path: '/webdav',
      root: RNFS.DocumentDirectoryPath + '/webdav'
    },
    {
      type: 'zip',
      path: '/archive',
      zip_file: RNFS.DocumentDirectoryPath + '/content.zip'
    },
    {
      type: 'static',
      path: '/static',
      root: staticDir,
      dir_list: {
        enabled: true,           // Enable directory listing
        show_hidden: false
      }
    },
    {
      type: 'upload',
      path: '/upload',
      temp_dir: RNFS.CachesDirectoryPath + '/uploads'
    },
    {
      type: 'buffer_upload',
      path: '/buffer-upload'
    },
    {
      type: 'rewrite',
      rules: [
        { pattern: '^/old/(.*)', replacement: '/static/$1' },
        { pattern: '^/api/v1/(.*)', replacement: '/api/v2/$1' }
      ]
    },
    {
      type: 'websocket',
      path: '/ws'
    }
  ],
  mime_types: {
    "myext": "application/x-custom-type" // Custom MIME type
  }
};

// Start server with plugin configuration
const server = await createConfigServer(8080, async (request) => {
  // Handle dynamic requests
  return {
    statusCode: 200,
    body: `API response for ${request.path}`,
  };
}, config);

// Now you can:
// - Access WebDAV at http://localhost:8080/webdav
// - Access zip content at http://localhost:8080/archive
// - Browse directories if index file is missing
// - Static files from staticDir
// - Dynamic API responses
// - WebSocket at ws://localhost:8080/ws
```

### CORS and Per-Mount Custom Headers

Enable CORS for your server and customize response headers for mounted plugins.

```typescript
import { createConfigServer, setCorsConfig } from 'react-native-nitro-http-server';

// Option 1: the `cors` field of the config server
await createConfigServer(port, handler, {
  cors: true, // or { origin: 'https://app.com', credentials: true, max_age: 600 }
  mounts: [
    {
      type: 'static',
      path: '/files',
      root: '/path/to/files',
      headers: { 'Cache-Control': 'no-cache', 'Access-Control-Allow-Origin': '*' },
    },
    { type: 'webdav', path: '/dav', root: '/path/to/dav' },
  ],
});

// Option 2: call setCorsConfig before starting other server types
setCorsConfig(true);
await createStaticServer(port, rootDir);
```

**Notes**:
- `setCorsConfig` / `config.cors` must be set before the server starts.
- `cors: true` enables the default behavior: `Access-Control-Allow-Origin: *`, and preflight (OPTIONS) requests are handled automatically.
- When `credentials: true`, `Access-Control-Allow-Origin` echoes the request `Origin`.
- The `headers` field on a mount allows overriding the plugin's default headers; it is a no-op for rewrite/websocket/upload mounts.

### WebSocket Server

Provides real-time bidirectional communication with full access to handshake information.

```typescript
import { ConfigServer } from 'react-native-nitro-http-server';

const server = new ConfigServer();

// Register WebSocket handler (before starting the server)
server.onWebSocket('/ws', (ws, request) => {
    // Access handshake info
    console.log('Path:', request.path);
    console.log('Query:', request.query);     // e.g., "token=abc&user=123"
    console.log('Headers:', request.headers); // Full HTTP handshake headers
    
    // Handle events
    ws.onmessage = (e) => {
        console.log('Received:', e.data);
        ws.send('Echo: ' + e.data);
    };
    
    ws.onclose = (e) => {
        console.log('Closed:', e.code, e.reason);
    };
});

// Start server with WebSocket mount
await server.start(8080, httpHandler, {
    mounts: [{ type: 'websocket', path: '/ws' }]
});
```

### Detecting client disconnects (`req.aborted` / `request.signal`)

When the client disconnects before your handler returns a response (tab closed, request
cancelled, network dropped), the JS side previously got **no notification at all** — you could
only notice indirectly by failing to send the response, or by waiting for
`request_timeout_secs`. Both paths now have a signal:

```typescript
// (1) High-level handler: use `signal` (recommended — you can hand it straight to fetch
//     or any downstream SDK's cancellation parameter)
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();
await server.start(8080, async (req) => {
    try {
        // This fetch is cancelled the moment the client goes away,
        // instead of waiting for its own timeout.
        const r = await fetch('https://upstream.example/api', { signal: req.signal });
        return { statusCode: 200, body: await r.text() };
    } catch (e) {
        if (req.signal?.aborted) return { statusCode: 499, body: 'client closed request' };
        throw e;
    }
});

// (2) Node-compatible layer: req.aborted / 'aborted' event / res.destroyed
import { createServer } from 'react-native-nitro-http-server';

const nodeServer = createServer((req, res) => {
    req.on('aborted', () => console.log('client left'));
    longQuery().then((data) => {
        if (req.aborted) return;          // nobody wants this result anymore
        res.end(data);
    });
});
```

Alignment with Node (every row below was measured against Node 22):

| Signal | This module | Node 22 |
| :--- | :--- | :--- |
| `req.aborted` | ✅ | ✅ |
| `req.on('aborted')` | ✅ | ✅ (deprecated since v17) |
| `req.destroyed` / `res.destroyed` | ✅ | ✅ |
| `res.on('close')` | ✅ | ✅ |
| `req.on('error')` (`code === 'ECONNRESET'`) | ✅ only when an `'error'` listener is **already attached** | ✅ likewise only with a listener |
| `request.signal` | ✅ **extension of this module** | ❌ not present on `http.IncomingMessage` |

⚠️ It fires **only when the response was never sent**: a normal completion, a handler
exception, and a request timeout all do **not** trigger it.

### RESTful API Example

```typescript
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();

// Mock database
const users = [
  { id: 1, name: 'Alice' },
  { id: 2, name: 'Bob' },
];

await server.start(8080, async (request) => {
  const { method, path } = request;
  
  // GET /api/users - Get all users
  if (method === 'GET' && path === '/api/users') {
    return {
      statusCode: 200,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(users),
    };
  }
  
  // GET /api/users/:id - Get a single user
  const userMatch = path.match(/^\/api\/users\/(\d+)$/);
  if (method === 'GET' && userMatch) {
    const userId = parseInt(userMatch[1]);
    const user = users.find(u => u.id === userId);
    
    if (user) {
      return {
        statusCode: 200,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(user),
      };
    } else {
      return {
        statusCode: 404,
        body: JSON.stringify({ error: 'User not found' }),
      };
    }
  }
  
  // POST /api/users - Create a new user
  if (method === 'POST' && path === '/api/users') {
    const newUser = JSON.parse(request.body || '{}');
    newUser.id = users.length + 1;
    users.push(newUser);
    
    return {
      statusCode: 201,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(newUser),
    };
  }
  
  // 404 - Route not found
  return {
    statusCode: 404,
    body: JSON.stringify({ error: 'Route not found' }),
  };
});
```

### Node.js Compatible API

Provides an interface compatible with Node.js `http` module, facilitating migration of existing code or using adapters for frameworks like Express/Koa.

```typescript
import { createServer } from 'react-native-nitro-http-server';

// Basic usage
const server = createServer((req, res) => {
  console.log(req.method, req.url);
  res.statusCode = 200;
  res.setHeader('Content-Type', 'text/plain');
  res.end('Hello from Node.js compatible API!');
});

server.listen(8080, () => {
  console.log('Server listening on port 8080');
});
```

**With `autoRestart`:**

```typescript
// Pass ServerOptions as the first argument
const server = createServer({ autoRestart: true }, (req, res) => {
  res.end('Hello');
}).listen(3000);

// Or using the Server class directly
import { Server } from 'react-native-nitro-http-server';
const server = new Server({ autoRestart: true });
server.on('request', (req, res) => res.end('Hello'));
server.listen(3000);
```

## 📖 API Documentation

### HttpServer

The basic HTTP server class for handling dynamic requests.

#### `start(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<number>`

Starts the HTTP server.

**Parameters**:
- `port`: Port number (1024-65535). Pass `0` to automatically allocate a random free port.
- `handler`: Request handler function, receives `HttpRequest` and returns `HttpResponse`
- `hostOrOptions`: (Optional) IP address string (e.g. `'0.0.0.0'`), or `ServerOptions` configuration object

**Returns**: The actual listening port number. returns `0` if failed.

**Example**:
```typescript
const actualPort = await server.start(0, async (request) => {
  return {
    statusCode: 200,
    body: 'Hello World',
  };
});
console.log(`Server started on port: ${actualPort}`);
```

#### `port: number` (Getter)

Returns the current listening port. Returns `0` if the server is not running.

#### `stop(): Promise<void>`

Stops the HTTP server.

**Example**:
```typescript
await server.stop();
```

#### `isRunning(): Promise<boolean>`

Checks if the HTTP server is running. Uses a TCP connect probe (200ms timeout) to detect the real socket state, rather than relying on an in-memory flag.

**Example**:
```typescript
const alive = await server.isRunning();
```

#### Auto-restart after background suspension

iOS suspends the app ~30s after screen lock, which tears down all TCP sockets. Enable `autoRestart` to let the library automatically detect and restart the server when the app returns to foreground — no extra code needed in your app.

```typescript
// Option 1: Using ServerOptions
await server.start(8080, handler, { autoRestart: true });

// Option 2: Set host and autoRestart together
await server.start(8080, handler, {
  host: '0.0.0.0',
  autoRestart: true,
});

// Option 3: Legacy host string (no autoRestart)
await server.start(8080, handler, '0.0.0.0');
```

> **How it works**: On entering background, a `beginBackgroundTask` request buys extra execution time (~30s–3min). On returning to foreground, a TCP connect probe checks if the socket is still alive — if alive, nothing happens; if dead, the server is cleaned up and restarted automatically.

### StaticServer

The static file server class.

#### `start(port: number, rootDir: string, hostOrOptions?: string | ServerOptions): Promise<number>`

Starts the static file server.

**Parameters**:
- `port`: Port number. Pass `0` for random port.
- `rootDir`: Absolute path to the static file root directory
- `hostOrOptions`: (Optional) IP address string, or `ServerOptions` configuration object

**Returns**: The actual listening port number. returns `0` if failed.

**Example**:
```typescript
import { StaticServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new StaticServer();
const success = await server.start(
  8080,
  RNFS.DocumentDirectoryPath + '/www'
);
```

#### `stop(): Promise<void>`

Stops the static file server.

#### `isRunning(): Promise<boolean>`

Checks if the static server is running. Uses a TCP connect probe to detect real socket state.

### AppServer

The app server class (hybrid mode) for both static files and dynamic requests.

#### `start(port: number, rootDir: string, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<number>`

Starts the app server (hybrid mode). The server will first attempt to find the corresponding static file in `rootDir`. If found and the method is GET, it returns the file content directly. Otherwise, it forwards the request to the `handler`.

**Parameters**:
- `port`: Port number
- `rootDir`: Static file root directory
- `handler`: Request handler
- `hostOrOptions`: (Optional) IP address string, or `ServerOptions` configuration object

#### `stop(): Promise<void>`

Stops the app server.

#### `isRunning(): Promise<boolean>`

Checks if the app server is running. Uses a TCP connect probe to detect real socket state.



### Type Definitions

#### HttpRequest

```typescript
interface HttpRequest {
  requestId: string;      // Unique request ID
  method: string;         // HTTP Method (GET, POST, PUT, DELETE, etc.)
  path: string;           // Request path
  headers: Record<string, string>;  // Request headers
  body?: string;          // Request body (optional)
  binaryBody?: ArrayBuffer; // Binary request body (used by buffer_upload)
}
```

#### HttpResponse

```typescript
interface HttpResponse {
  statusCode: number;     // HTTP Status Code (200, 404, 500, etc.)
  headers?: Record<string, string>;  // Response headers (optional)
  body?: string | ArrayBuffer;       // Response body (string or ArrayBuffer)
}
```

#### `stopAppServer(): Promise<void>`

Stops the app server.

### ConfigServer

Server with plugin configuration support (WebDAV, Zip mounting, etc.).

#### `start(port: number, handler: RequestHandler, config: ServerConfig, hostOrOptions?: string | ServerOptions): Promise<number>`

Starts the config server with plugin configuration.

**Parameters**:
- `port`: Port number
- `handler`: Request handler
- `config`: Plugin configuration object (includes `root_dir`)
- `hostOrOptions`: (Optional) IP address string, or `ServerOptions` configuration object

**Example**:
```typescript
const config = {
  root_dir: staticDir,
  mounts: [
    {
      type: 'webdav',
      path: '/webdav',
      root: RNFS.DocumentDirectoryPath + '/webdav'
    },
    {
      type: 'zip',
      path: '/archive',
      zip_file: RNFS.DocumentDirectoryPath + '/content.zip'
    }
  ]
};

const server = new ConfigServer();
await server.start(8080, handler, config, { host: '0.0.0.0' });
```

#### `stop(): Promise<void>`

Stops the config server.

#### `isRunning(): Promise<boolean>`

Checks if the config server is running. Uses a TCP connect probe to detect real socket state.

### Helper Functions

#### `createHttpServer(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<HttpServer>`

Creates and starts a basic HTTP server.

#### `createStaticServer(port: number, rootDir: string, hostOrOptions?: string | ServerOptions): Promise<StaticServer>`

Creates and starts a static file server.

#### `createAppServer(port: number, rootDir: string, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<AppServer>`

Creates and starts an app server (hybrid mode).

#### `createConfigServer(port: number, handler: RequestHandler, config: ServerConfig, hostOrOptions?: string | ServerOptions): Promise<ConfigServer>`

Creates and starts a config server with plugin configuration.

#### `setCorsConfig(config: boolean | CorsConfig): void`

Sets the global CORS configuration for all server types (`HttpServer`, `StaticServer`, `AppServer`, Node.js compatible `http.createServer`). Call before starting the server. `true` enables the default configuration (`Access-Control-Allow-Origin: *`, automatic preflight handling), an object customizes the behavior, `false` disables CORS.

### Type Definitions

#### HttpRequest

```typescript
interface HttpRequest {
  requestId: string;      // Unique request ID
  method: string;         // HTTP Method (GET, POST, PUT, DELETE, etc.)
  path: string;           // Request path
  headers: Record<string, string>;  // Request headers
  body?: string;          // Request body (optional)
  binaryBody?: ArrayBuffer; // Binary request body (used by buffer_upload)
}
```

#### HttpResponse

```typescript
interface HttpResponse {
  statusCode: number;     // HTTP Status Code (200, 404, 500, etc.)
  headers?: Record<string, string>;  // Response headers (optional)
  body?: string | ArrayBuffer;       // Response body (string or ArrayBuffer)
}
```

#### ServerOptions

```typescript
interface ServerOptions {
  /** IP address to bind to, defaults to 127.0.0.1 */
  host?: string
  /**
   * Automatically detect and restart the server after iOS background suspension.
   * Defaults to false. Set to true to enable zero-config auto-recovery.
   */
  autoRestart?: boolean
}
```

#### ServerConfig

```typescript
interface ServerConfig {
  root_dir?: string;             // Static file root (Optional, as default static mount)
  verbose?: boolean | 'off' | 'error' | 'warn' | 'info' | 'debug'; // Log level (default: 'off')
  cors?: boolean | CorsConfig;   // CORS config: true = default (Allow-Origin: *), object to customize, omitted = disabled
  mime_types?: MimeTypesConfig;
  mounts?: Mountable[];          // Unified mount list
  /**
   * How long to wait for the JS handler to return a response, in seconds (default 30).
   * Long-polling / SSE need a larger value, otherwise the connection is cut and answered 500.
   *
   * ⚠️ **Sticky global** (see Known Limitations): only written when you pass this field, so a
   * value set by an earlier server keeps applying to later servers that omit it.
   * Pass 30 explicitly to reset it.
   */
  request_timeout_secs?: number
  /**
   * Request body limit for the callback path, in bytes (default 100MB). Over the limit the
   * request is answered **413** and your handler is never called.
   * Plugin paths (static / zip / upload / webdav) do not read the body and are not affected.
   *
   * ⚠️ **Sticky global** (see Known Limitations): same caveat as `request_timeout_secs`.
   * Pass `100 * 1024 * 1024` explicitly to reset it.
   */
  max_body_size?: number
}

interface CorsConfig {
  origin?: string;        // default "*"
  methods?: string[];     // default: common methods + WebDAV methods
  headers?: string[];     // default ["*"]; echoes the request's Access-Control-Request-Headers on preflight
  credentials?: boolean;  // default false; when true, Allow-Origin echoes the request Origin
  max_age?: number;       // preflight cache duration in seconds
}

type Mountable = WebDavMount | ZipMount | StaticMount | UploadMount | BufferUploadMount | RewriteMount | WebSocketMount;

// All mounts except RewriteMount also accept:
headers?: Record<string, string>; // Custom response headers; overrides plugin default headers (no-op for rewrite/websocket/upload mounts)

interface WebDavMount {
  type: 'webdav';
  path: string;      // Mount path, e.g., "/webdav"
  root: string;      // WebDAV root directory
}

interface ZipMount {
  type: 'zip';
  path: string;      // Mount path, e.g., "/zip"
  zip_file: string;  // Zip file path
}

interface UploadMount {
  type: 'upload';
  path: string;      // Mount path, e.g., "/upload"
  temp_dir: string;  // Temporary directory for uploaded files
}

interface BufferUploadMount {
  type: 'buffer_upload';
  path: string;      // Mount path, e.g., "/buffer-upload"
}

interface RewriteMount {
  type: 'rewrite';
  rules: RewriteRule[];
}

interface WebSocketMount {
  type: 'websocket';
  path: string;              // WebSocket endpoint, e.g., "/ws"
  max_message_size?: number; // Max message size in bytes (default: 64MB)
}

// WebSocket Connection Request
interface WebSocketConnectionRequest {
  path: string;                      // Connection path
  query: string;                     // Query string
  headers: Record<string, string>;   // HTTP handshake headers
}

// WebSocket Connection Handler
type WebSocketConnectionHandler = (
  ws: ServerWebSocket, 
  request: WebSocketConnectionRequest
) => void;

interface RewriteRule {
  pattern: string;      // Regex pattern
  replacement: string;  // Replacement string (supports $1, $2...)
}

interface StaticMount {
  type: 'static';
  path: string;      // Mount path, e.g., "/images"
  root: string;      // Local file system directory
  dir_list?: DirListConfig;
  default_index?: string[];
}

type MimeTypesConfig = Record<string, string>;

interface DirListConfig {
  enabled: boolean;        // Enable directory listing
  show_hidden?: boolean;   // Show hidden files (default: false)
}
```

#### RequestHandler

```typescript
type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse;
```

The request handler can return a Promise or a response object directly.

### Node.js Compatible Layer

Exports the following objects and functions compatible with Node.js `http` module:

- `createServer(options?: ServerOptions, requestListener?: (req: IncomingMessage, res: ServerResponse) => void): Server`
- `createServer(requestListener?: (req: IncomingMessage, res: ServerResponse) => void): Server`
- `Server` class — constructor accepts `new Server(options?: ServerOptions, requestListener?)`
- `IncomingMessage` class
- `ServerResponse` class
- `STATUS_CODES`
- `METHODS`

#### `ServerOptions` (Node.js Compatible)

```typescript
interface ServerOptions {
  IncomingMessage?: typeof IncomingMessage;
  ServerResponse?: typeof ServerResponse;
  requestTimeout?: number;      // default: 300000
  headersTimeout?: number;      // default: 60000
  keepAliveTimeout?: number;    // default: 5000
  /** Automatically detect and restart after iOS background suspension. Default false. */
  autoRestart?: boolean;
}
```

#### Streaming APIs

The library also provides low-level streaming APIs for advanced use cases:

- `readRequestBodyChunk(requestId: string): Promise<string>` - Read request body in chunks
- `writeResponseChunk(requestId: string, chunk: string): Promise<boolean>` - Write response body in chunks
- `endResponse(requestId: string, statusCode: number, headersJson: string): Promise<boolean>` - End streaming response
- `sendBinaryResponse(requestId: string, statusCode: number, headersJson: string, body: ArrayBuffer): Promise<boolean>` - Send binary response

These APIs are used internally by the Node.js compatible layer for streaming support.

## 🏗️ Architecture

```
┌─────────────────────────────────────┐
│      JavaScript / TypeScript        │
│      (React Native App)             │
└──────────────┬──────────────────────┘
               │ Nitro Modules
┌──────────────┴──────────────────────┐
│      C++ Bridge Layer               │
│      (HybridHttpServer)             │
└──────────────┬──────────────────────┘
               │ FFI (C ABI)
┌──────────────┴──────────────────────┐
│      Rust Core                      │
│      (Actix-web + Tokio)            │
└─────────────────────────────────────┘
```

### Tech Stack

- **JavaScript Layer**: TypeScript, React Native
- **Bridge Layer**: Nitro Modules (C++)
- **Core Layer**: Rust (Actix-web, Tokio)

### Data Flow

1.  **Request Arrival**: Rust Actix-web server receives HTTP request.
2.  **C Callback**: Calls C callback function via FFI.
3.  **C++ Conversion**: C++ converts C struct to Nitro types.
4.  **JavaScript Call**: Calls JavaScript handler via Nitro Modules.
5.  **Response Return**: JavaScript returns response → C++ → C → Rust → HTTP Client.

## ⚠️ Known Limitations

Intentional behavior, not-yet-implemented features, and platform constraints. Each item
states **what you will observe** and **what to do instead**.

### `server.close()` is asynchronous (stop→start ordering is handled for you)

`close()` only **starts** the native stop and returns immediately; the `'close'` event and
the `close(cb)` callback fire after the native side has actually finished.

Because the native server is a singleton, a `start()` landing in that window used to race
with the teardown: the new server reported a port, but its requests got **no response at
all** (a connect/read failure, not a 500) — intermittently, so it looked like a flaky test.

**Fixed 2026-09-22** — `start()` now waits for any in-flight native stop before proceeding
(`src/nativeStopQueue.ts`), so the Node-style pattern is safe again:

```typescript
oldServer.close();
newServer.listen(8080);   // ✓ safe — start() waits for the pending stop
```

Verified on a real device (the on-device RN test suite, test D11, which deliberately does the
racy pattern). You may still `await new Promise(r => old.close(() => r()))` if you want an
explicit ordering guarantee in your own code; it costs nothing.

### `request.signal` requires the host to provide `AbortController`

`request.signal` (and the `AbortController` behind it) **only exists when the host implements
it**:

- **React Native does** — `Libraries/Core/setUpXHR.js` calls
  `polyfillGlobal('AbortController', …)`, so it works on device;
- **headless JS hosts generally do not** — on the headless host this module is verified
  against, `typeof globalThis.AbortController === 'undefined'`.

On such a host `req.signal` is simply `undefined`, and **every other disconnect signal keeps
working** (`req.aborted` / `'aborted'` / `res.destroyed` do not depend on `AbortController`).

→ Guard before use: `if (req.signal) { ... }`, or use optional chaining `req.signal?.aborted`.
Detect it with `typeof AbortController === 'undefined'`.

The module deliberately ships **no** internal fallback: a hand-rolled signal that only runs on
a test host would mean the tests are not exercising the real thing.

### Multiple callback servers share one native handler

`HttpServer` / `AppServer` / `ConfigServer` all register their handler into a single global
slot. Starting a second one **overwrites** the first, so every request — on *any* port — is
routed to the **last started** handler.

```typescript
await serverA.start(8080, handlerA);   // handlerA is live
await serverB.start(8081, handlerB);   // now *both* ports call handlerB
```

- **Do**: run one callback server and split traffic with `ConfigServer` mounts
  (`static` / `zip` / `upload` / `rewrite` / `websocket`).
- Static-only servers are unaffected — `StaticServer` never uses the callback slot.

### Node.js compatible layer: the timeout options are inert

`server.requestTimeout` / `headersTimeout` / `keepAliveTimeout` (and `server.timeout`) are
plain local properties of the Node-compat `Server`. They are readable and writable, but
**nothing reads them** — changing them has no effect.

Use the config server instead: `request_timeout_secs` (seconds, default 30).

### Node.js compatible layer: bodies go through a UTF-8 string channel

`HttpRequest.body` and `HttpResponse.body` are `string`, so any non-UTF-8 byte sequence is
mangled on the way in or out. Do **not** push binary through them.

Two escape hatches:

| Direction | How |
| :--- | :--- |
| Request | `request.binaryBody` (`ArrayBuffer`) — populated **only** for `buffer_upload` pass-through requests (those carrying an `x-upload-filename` header) |
| Response | `res.end(arrayBuffer)` on the Node-compat layer, or return an `ArrayBuffer` body from a `createHttpServer` handler |

⚠️ Known gap: `res.write('a'); res.end(arrayBuffer)` **silently drops** the `'a'` — the
binary path sends an explicit body and never reads the chunk accumulator; serializing the
writes only guarantees ordering. Fixing it needs a Rust-side change.

### Upload plugins: filename headers are percent-encoded when non-ASCII

The two upload plugins use **different header names**:

| Plugin | Filename header | Marker header |
| :--- | :--- | :--- |
| `buffer_upload` | `X-Upload-Filename` | `X-Upload-Filename-Encoding: percent` |
| `upload` | `X-Uploaded-Original-Name` | `X-Uploaded-Filename-Encoding: percent` |

The value is percent-encoded **only when the name is not header-readable** (non-ASCII), and
the marker header is present **only in that case**. Check the marker before decoding —
blindly calling `decodeURIComponent` corrupts names that legitimately contain `%`:

```typescript
const name = request.headers['x-uploaded-original-name'];
const filename = request.headers['x-uploaded-filename-encoding'] === 'percent'
  ? decodeURIComponent(name)
  : name;
```

### `upload` mount: temp files are never cleaned up, and only the first file is reported

- Files are written into `temp_dir` and **left there** — the plugin never deletes them. Put
  `temp_dir` somewhere the OS reclaims (e.g. the caches directory) and clean it yourself.
- A `multipart` request with several files is accepted, but only the **first** file is
  reported in the headers (`x-uploaded-file-path` etc.). Loop over the request if you need
  all of them.

### `request_timeout_secs` / `max_body_size` are **sticky** globals

Both are stored in native globals that are only written **when you pass the field**
(`global.rs`, `app_server.rs`). Nothing resets them on `stop()`, so a value set by one
server keeps applying to every later server — even one that omits the field entirely:

```typescript
await cfgA.start(0, h, { max_body_size: 1024 });   // 1 KB limit
await cfgA.stop();
await HttpServerModule.start(0, h);                // no config at all — still 1 KB!
// → a 1 MB POST now answers 413
```

This is easy to hit when one screen uses a small `max_body_size` for upload tests and
another expects the default. **Pass the value you want every time** (or reset it
explicitly) if your app starts more than one server.

> Found by the on-device RN test suite — the 1 MB POST in the
> "core" suite started returning 413 after the mounts suite had set `max_body_size: 1024`.

### `zip` mounts hold the whole archive in memory

A zip mount reads the archive once and keeps it in memory (zero file I/O per request).
Fine for asset bundles, **not** for archives larger than roughly 100MB.

### WebSocket `send()` can return `false`

`ws.send()` returns `false` when the message was **not** enqueued: the connection is gone,
or the per-connection send queue (256 messages) is full. The message is not queued and not
retried — the caller decides:

```typescript
if (!(await ws.send(data))) {
  // back off and retry, or close the connection
}
```

### `getStats()` — only 3 of its 6 fields are real

| Field | Status |
| :--- | :--- |
| `totalRequests` | **real** |
| `errorCount` | **real** — server-side failures only (callback timeout, response channel closed, static plugin I/O error). A 4xx/5xx returned by your own handler does **not** count |
| `uptime` | **real** — seconds since the last successful start |
| `activeConnections` | always `0` — hyper 0.14's `Server::serve` exposes no per-connection hook |
| `bytesSent` / `bytesReceived` | always `0` — responses leave through more than one path, and counting only some of them would be worse than reporting `0` |

### Static file serving has no Range / conditional requests

`Range`, `ETag` and `If-Modified-Since` are not implemented — every response is a full
`200`. Put a CDN or reverse proxy in front if you need them.

### Multiple WebSocket mounts: only the last one is reachable

The WebSocket plugin is a global singleton, so with several `websocket` mounts configured
only the last one receives upgrades.

### Don't mix `onWebSocket()` with the standalone `setupWebSocketHandler()`

Both install into the same **global single** native callback (last writer wins), and
`ConfigServer` re-installs a handler on auto-restart. Pick one:

- `server.onWebSocket('/ws', handler)` — path-based dispatch (recommended)
- `setupWebSocketHandler(handler)` — a single handler for everything

If you do mix them, note that auto-restart preserves whichever one was installed last; it
no longer silently switches owners.

## 🔧 FAQ

### Q: Why does the server fail to start?

**A**: Possible reasons:
1.  **Port In Use**: Try changing the port number.
2.  **Insufficient Permissions**: Some ports (like 80, 443) require root privileges.
3.  **Firewall**: Check firewall settings.

### Q: How to handle large file uploads?

**A**: The `body` field in the current version is a string type, which is not suitable for large files. Suggestions:
- **Use the `UploadPlugin` (Recommended)**: Configure an `upload` mount. It intercepts multipart uploads, saves files to a temporary directory, and injects file paths into request headers (`x-uploaded-file-path`), keeping the JS payload light.
- **Use the `BufferUploadPlugin`**: Process files in memory (limit 100MB). Access data via `request.binaryBody`.
- Use the static file server (for downloads).
- Add streaming support in the Rust layer (advanced).

### Q: Is HTTPS supported?

**A**: The current version does not directly support HTTPS. It is recommended to use a reverse proxy (like Nginx) to provide HTTPS support.

### Q: How is the performance?

**A**: Built on Rust's Actix-web framework, performance is excellent. Here are the benchmark results (Test Environment: MacMini M4, 1 Thread, 2 Connections):

| Mode | QPS (Req/Sec) | Latency (Avg) |
| :--- | :--- | :--- |
| **Basic HTTP** | **~41.85k** | **~58.14us** |
| **Node.js Compatible API** | **~21.60k** | **~274.81us** |
| **Koa Framework** | **~13.32k** | **~313.10us** |
| **Binary Mode** | **~35.46k** | **~124.29us** |

*Note: The Node.js compatible layer has lower performance due to additional JavaScript bridging and object conversion, but it is still sufficient for most application scenarios.*

### Q: Can I run dynamic and static servers simultaneously?

**A**: Yes. You can either start the dynamic server and static server separately (using different ports) or use `startAppServer` to provide both static file and dynamic API services on the same port.

```typescript
// Method 1: Use startAppServer (Recommended)
await server.startAppServer(8080, staticDir, apiHandler);

// Method 2: Start separately (Different ports)
await server.start(8080, handler);

// Static server on 8081
await server.startStaticServer(8081, staticDir);
```

> ⚠️ This works because `StaticServer` never uses the callback slot. **Two *callback*
> servers** (`HttpServer` / `AppServer` / `ConfigServer`) are a different story — they share
> one global handler and the last one started wins. See
> [Known Limitations](#multiple-callback-servers-share-one-native-handler).

### Q: How to debug server issues?

**A**:
1.  Check server logs (Xcode/Logcat).
2.  Use `getStats()` to view statistics.
3.  Use tools to test (curl, Postman).

```bash
# Test server
curl http://localhost:8080/api/test
```

## 📝 Changelog

### Unreleased

- **New: the JS side is now notified when the client disconnects.** Previously, if the client
  went away before the handler returned a response, JS got no signal at all — you could only
  notice indirectly by failing to send the response or by waiting out `request_timeout_secs`.
  There are now two paths: high-level handlers get `request.signal` (hand it straight to
  `fetch` or any downstream SDK's cancellation parameter), and the Node-compatible layer gets
  `req.aborted` / the `'aborted'` event / `res.destroyed`. Every signal except
  `request.signal` (an extension of this module — Node's `IncomingMessage` has no such
  property) was **measured against Node 22** and matches. It fires **only when the response
  was never sent** — a normal completion, a handler exception, and a request timeout do not
  trigger it. Implemented across four layers (Rust predicate → C header → C++ callback →
  Nitro thread hop → TS).
- **Fixed the `close()` → `start()` race.** The native server is a **singleton**, and
  `close()` only *starts* the native stop (the `'close'` callback fires once the native side
  has actually finished). A `start()` landing in that window raced with the teardown: the new
  server reported a port, but its requests got **no response at all** (a connect/read
  failure, not a 500) — intermittently, so it looked like a flaky test. `start()` now waits
  for any in-flight native stop first (`src/nativeStopQueue.ts`). Verified on device
  (the on-device RN test suite, test D11).
- **Docs:** `request_timeout_secs` / `max_body_size` are documented as **sticky globals** —
  they are only written when you pass the field and `stop()` does not reset them, so a value
  set by one server keeps applying to later servers that omit it. Found by the on-device
  suite (a 1 MB POST suddenly answering 413).

### 1.9.0

- 🔄 `start()` methods now accept `ServerOptions` config object (backward-compatible with legacy `host` string)
- 🔁 Added `autoRestart` option: automatically detect and restart server after iOS background suspension
- 🔍 `isRunning()` is now async — uses a TCP connect probe to detect real socket state
- 🛡️ iOS background task: requests extra execution time on entering background

### 1.8.0 (2026-03-18)

- ✨ Support **random port allocation** by passing `0` as the port number.
- 🔄 Enhanced all server `start` methods to return the **actual listening port** (number) instead of a boolean.
- 🏗️ Added `port` property to all server classes to retrieve the current listening port.
- 🛡️ Improved Rust core stability by preventing panics during port binding and ensuring compatibility with Tokio runtime in multi-threaded environments.

### 1.0.0 (2025-12-08)

- 🎉 Initial release.
- ✅ Full implementation based on Nitro Modules.
- ✅ Dynamic request handling support.
- ✅ Static file serving support.
- ✅ iOS and Android support.

## 📄 License

ISC

## 🤝 Contribution

Issues and Pull Requests are welcome!

## 🔗 Related Links

- [Nitro Modules](https://github.com/mrousavy/nitro)
- [Actix-web](https://actix.rs/)
- [React Native](https://reactnative.dev/)

---

**Made with ❤️ using Rust, C++, and React Native**
