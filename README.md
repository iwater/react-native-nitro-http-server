# React Native HTTP Server

A high-performance React Native HTTP server library, implemented in Rust, supporting dynamic request handling and static file serving.

## ✨ Features

- 🚀 **High Performance**: Built on Rust's Actix-web framework, delivering exceptional performance.
- 📱 **Cross-Platform**: Supports iOS and Android.
- 🔄 **Asynchronous**: Uses Nitro Modules to provide native async APIs.
- 📁 **Static File Serving**: Built-in static file server support.
- 🎯 **Easy to Use**: TypeScript-friendly API design.
- ⚡ **Zero Copy**: Direct FFI calls to Rust code.
- 🔌 **Plugin System**: Support for WebDAV, Zip mounting, and extensible plugins.
- 🌊 **Streaming APIs**: Support for streaming request/response bodies.
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

// Start server
await server.start(8080, async (request) => {
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

console.log('Server running at http://localhost:8080');

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
  webdav: {
    prefix: '/webdav',           // WebDAV path prefix
    root: RNFS.DocumentDirectoryPath + '/webdav'
  },
  zip_mount: {
    prefix: '/archive',          // Zip mount path prefix
    zip_file: RNFS.DocumentDirectoryPath + '/content.zip'
  },
  mime_types: {
    "myext": "application/x-custom-type" // Custom MIME type
  }
};

// Start server with plugin configuration
const server = await createConfigServer(8080, staticDir, async (request) => {
  // Handle dynamic requests
  return {
    statusCode: 200,
    body: `API response for ${request.path}`,
  };
}, config);

// Now you can:
// - Access WebDAV at http://localhost:8080/webdav
// - Access zip content at http://localhost:8080/archive
// - Static files from staticDir
// - Dynamic API responses
```

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

## 📖 API Documentation

### HttpServer

The basic HTTP server class for handling dynamic requests.

#### `start(port: number, handler: RequestHandler): Promise<boolean>`

Starts the HTTP server.

**Parameters**:
- `port`: Port number (1024-65535)
- `handler`: Request handler function, receives `HttpRequest` and returns `HttpResponse`

**Returns**: `true` if started successfully.

**Example**:
```typescript
const success = await server.start(8080, async (request) => {
  return {
    statusCode: 200,
    body: 'Hello World',
  };
});
```

#### `stop(): Promise<void>`

Stops the HTTP server.

**Example**:
```typescript
await server.stop();
```

### StaticServer

The static file server class.

#### `start(port: number, rootDir: string, host?: string): Promise<boolean>`

Starts the static file server.

**Parameters**:
- `port`: Port number
- `rootDir`: Absolute path to the static file root directory
- `host`: (Optional) IP address to bind to, defaults to `127.0.0.1`

**Returns**: `true` if started successfully.

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

#### `isRunning(): boolean`

Checks if the static server is running.

### AppServer

The app server class (hybrid mode) for both static files and dynamic requests.

#### `start(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<boolean>`

Starts the app server (hybrid mode). The server will first attempt to find the corresponding static file in `rootDir`. If found and the method is GET, it returns the file content directly. Otherwise, it forwards the request to the `handler`.

**Parameters**:
- `port`: Port number
- `rootDir`: Static file root directory
- `handler`: Request handler
- `host`: (Optional) IP address to bind to, defaults to `127.0.0.1`

#### `stop(): Promise<void>`

Stops the app server.

#### `isRunning(): boolean`

Checks if the app server is running.



### Type Definitions

#### HttpRequest

```typescript
interface HttpRequest {
  requestId: string;      // Unique request ID
  method: string;         // HTTP Method (GET, POST, PUT, DELETE, etc.)
  path: string;           // Request path
  headers: Record<string, string>;  // Request headers
  body?: string;          // Request body (optional)
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

#### `start(port: number, rootDir: string, handler: RequestHandler, config: ServerConfig, host?: string): Promise<boolean>`

Starts the config server with plugin configuration.

**Parameters**:
- `port`: Port number
- `rootDir`: Static files root directory
- `handler`: Request handler
- `config`: Plugin configuration object
- `host`: (Optional) IP address to bind to, defaults to `127.0.0.1`

**Example**:
```typescript
const config = {
  webdav: {
    prefix: '/webdav',
    root: RNFS.DocumentDirectoryPath + '/webdav'
  },
  zip_mount: {
    prefix: '/archive',
    zip_file: RNFS.DocumentDirectoryPath + '/content.zip'
  }
};

const server = new ConfigServer();
await server.start(8080, staticDir, handler, config, '0.0.0.0');
```

#### `stop(): Promise<void>`

Stops the config server.

#### `isRunning(): boolean`

Checks if the config server is running.

### Helper Functions

#### `createHttpServer(port: number, handler: RequestHandler, host?: string): Promise<HttpServer>`

Creates and starts a basic HTTP server.

#### `createStaticServer(port: number, rootDir: string, host?: string): Promise<StaticServer>`

Creates and starts a static file server.

#### `createAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<AppServer>`

Creates and starts an app server (hybrid mode).

#### `createConfigServer(port: number, rootDir: string, handler: RequestHandler, config: ServerConfig, host?: string): Promise<ConfigServer>`

Creates and starts a config server with plugin configuration.

### Type Definitions

#### HttpRequest

```typescript
interface HttpRequest {
  requestId: string;      // Unique request ID
  method: string;         // HTTP Method (GET, POST, PUT, DELETE, etc.)
  path: string;           // Request path
  headers: Record<string, string>;  // Request headers
  body?: string;          // Request body (optional)
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

#### ServerConfig

```typescript
interface ServerConfig {
  webdav?: WebDavConfig;
  zip_mount?: ZipMountConfig;
  mime_types?: MimeTypesConfig;
}

interface WebDavConfig {
  prefix: string;    // Path prefix, e.g., "/webdav"
  root: string;   // WebDAV root directory
}

interface ZipMountConfig {
  prefix: string;    // Path prefix, e.g., "/zip"
  zip_file: string;   // Path to zip file
}

type MimeTypesConfig = Record<string, string>; // extension -> mime-type mapping
```

#### RequestHandler

```typescript
type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse;
```

The request handler can return a Promise or a response object directly.

### Node.js Compatible Layer

Exports the following objects and functions compatible with Node.js `http` module:

- `createServer(requestListener?: (req: IncomingMessage, res: ServerResponse) => void): Server`
- `Server` class
- `IncomingMessage` class
- `ServerResponse` class
- `STATUS_CODES`
- `METHODS`

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

## 🔧 FAQ

### Q: Why does the server fail to start?

**A**: Possible reasons:
1.  **Port In Use**: Try changing the port number.
2.  **Insufficient Permissions**: Some ports (like 80, 443) require root privileges.
3.  **Firewall**: Check firewall settings.

### Q: How to handle large file uploads?

**A**: The `body` field in the current version is a string type, which is not suitable for large files. Suggestions:
- Use the static file server.
- Add streaming support in the Rust layer.

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

### 1.0.0 (2025-12-08)

- 🎉 Initial release.
- ✅ Full implementation based on Nitro Modules.
- ✅ Dynamic request handling support.
- ✅ Static file serving support.
- ✅ iOS and Android support.

## 📄 License

MIT

## 🤝 Contribution

Issues and Pull Requests are welcome!

## 🔗 Related Links

- [Nitro Modules](https://github.com/mrousavy/nitro)
- [Actix-web](https://actix.rs/)
- [React Native](https://reactnative.dev/)

---

**Made with ❤️ using Rust, C++, and React Native**
