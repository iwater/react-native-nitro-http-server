# React Native HTTP Server

一个高性能的 React Native HTTP 服务器库，基于 Rust 实现，支持动态请求处理和静态文件服务。

## ✨ 特性

- 🚀 **高性能**: 基于 Rust 的 Actix-web 框架，性能卓越
- 📱 **跨平台**: 支持 iOS 和 Android
- 🔄 **异步处理**: 使用 Nitro Modules 提供原生异步 API
- 📁 **静态文件服务**: 内置静态文件服务器支持
- 📂 **目录列表**: 自动生成目录列表页面
- 🎯 **简单易用**: TypeScript 友好的 API 设计
- ⚡ **零拷贝**: 直接通过 FFI 调用 Rust 代码
- 🔌 **插件系统**: 支持 WebDAV、Zip 挂载等可扩展插件
- 🌊 **流式 API**: 支持流式请求/响应体处理
- 📤 **文件上传插件**: 支持高效处理 `multipart/form-data` 文件上传（保存到磁盘）
- 💾 **Buffer Upload 插件**: 在内存中处理文件上传，支持直接访问 `ArrayBuffer`
- 🔀 **URL 重写插件**: 支持基于正则表达式的 URL 重写
- 🔌 **WebSocket 插件**: 实时双向通信，支持获取完整的握手信息
- 🔄 **Node.js 兼容**: 兼容 Node.js `http` 模块 API

## 📦 安装

```bash
npm install react-native-nitro-http-server
# 或
yarn add react-native-nitro-http-server
```

### iOS 配置

运行 pod install:

```bash
cd ios && pod install
```

### Android 配置

无需额外配置，自动链接。

## 🚀 快速开始

### 基础 HTTP 服务器

```typescript
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();

// 启动服务器（返回实际端口号，例如 8080）
const actualPort = await server.start(8080, async (request) => {
  console.log(`收到请求: ${request.method} ${request.path}`);
  
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

console.log(`服务器运行在 http://localhost:${actualPort}`);

// 传入 0 可以自动分配一个随机空闲端口
const randomPort = await server.start(0, handler);
console.log(`服务器启动在随机端口: ${randomPort}`);

// 获取当前端口
console.log('当前端口:', server.port);

// 停止服务器
// await server.stop();
```

### 二进制响应示例

```typescript
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();

await server.start(8080, async (request) => {
  // 返回二进制图片
  const imageBuffer = new ArrayBuffer(1024); // 您的二进制数据
  
  return {
    statusCode: 200,
    headers: {
      'Content-Type': 'image/png',
    },
    body: imageBuffer, // 直接支持 ArrayBuffer
  };
});
```

### 静态文件服务器

```typescript
import { StaticServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new StaticServer();

// 启动静态文件服务器
const staticDir = RNFS.DocumentDirectoryPath + '/www';
await server.start(8080, staticDir);

console.log(`静态文件服务器运行在 http://localhost:8080`);
console.log(`服务目录: ${staticDir}`);

// 停止静态服务器
// await server.stop();
```

### 应用服务器 (混合模式)

同时支持静态文件服务和动态 API 处理。优先尝试服务静态文件，如果文件不存在则调用回调函数。

```typescript
import { AppServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new AppServer();
const staticDir = RNFS.DocumentDirectoryPath + '/www';

// 启动应用服务器（混合模式）
await server.start(8080, staticDir, async (request) => {
  // 静态文件不存在时会执行此回调
  return {
    statusCode: 200,
    body: `Dynamic response for ${request.path}`,
  };
});
```

### 配置服务器 (带插件)

通过插件配置支持 WebDAV、Zip 文件挂载等高级功能。

```typescript
import { createConfigServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const staticDir = RNFS.DocumentDirectoryPath + '/www';

// 配置插件
const config = {
  root_dir: staticDir,           // 静态文件根目录（可选，作为默认静态挂载点）
  verbose: 'info',               // 日志等级: 'off' | 'error' | 'warn' | 'info' | 'debug' (默认 'off')
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
        enabled: true,           // 启用目录列表
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
    "myext": "application/x-custom-type" // 自定义 MIME 类型
  }
};

// 启动带插件配置的服务器
const server = await createConfigServer(8080, async (request) => {
  // 处理动态请求
  return {
    statusCode: 200,
    body: `API 响应: ${request.path}`,
  };
}, config);

// 现在可以：
// - 通过 http://localhost:8080/webdav 访问 WebDAV
// - 通过 http://localhost:8080/archive 访问 zip 文件内容
// - 如果缺少索引文件，可以浏览目录
// - 访问 staticDir 中的静态文件
// - 获得动态 API 响应
// - 通过 ws://localhost:8080/ws 连接 WebSocket
```

### WebSocket 服务器

提供实时双向通信，支持获取完整的握手信息。

```typescript
import { ConfigServer } from 'react-native-nitro-http-server';

const server = new ConfigServer();

// 注册 WebSocket 处理器（必须在 start 之前调用）
server.onWebSocket('/ws', (ws, request) => {
    // 获取握手信息
    console.log('Path:', request.path);
    console.log('Query:', request.query);     // 如 "token=abc&user=123"
    console.log('Headers:', request.headers); // 完整的 HTTP 握手头
    
    // 处理事件
    ws.onmessage = (e) => {
        console.log('收到:', e.data);
        ws.send('回复: ' + e.data);
    };
    
    ws.onclose = (e) => {
        console.log('关闭:', e.code, e.reason);
    };
});

// 启动带 WebSocket 配置的服务器
await server.start(8080, httpHandler, {
    mounts: [{ type: 'websocket', path: '/ws' }]
});
```

### RESTful API 示例

```typescript
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();

// 模拟数据库
const users = [
  { id: 1, name: 'Alice' },
  { id: 2, name: 'Bob' },
];

await server.start(8080, async (request) => {
  const { method, path } = request;
  
  // GET /api/users - 获取所有用户
  if (method === 'GET' && path === '/api/users') {
    return {
      statusCode: 200,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(users),
    };
  }
  
  // GET /api/users/:id - 获取单个用户
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
  
  // POST /api/users - 创建新用户
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
  
  // 404 - 路由未找到
  return {
    statusCode: 404,
    body: JSON.stringify({ error: 'Route not found' }),
  };
});
```

### Node.js 兼容 API

提供与 Node.js `http` 模块兼容的接口，方便迁移现有代码或使用 Express/Koa 等框架的适配器。

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

## 📖 API 文档

### HttpServer

基础 HTTP 服务器类，用于处理动态请求。

#### `start(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<number>`

启动 HTTP 服务器。

**参数**:
- `port`: 端口号（1024-65535）。传入 `0` 将自动分配一个随机空闲端口。
- `handler`: 请求处理函数,接收 `HttpRequest` 并返回 `HttpResponse`
- `hostOrOptions`: (可选) 监听的IP地址字符串（如 `'0.0.0.0'`），或 `ServerOptions` 配置对象

**返回**: 实际监听的端口号。如果启动失败返回 `0`

**示例**:
```typescript
// 自动分配端口
const actualPort = await server.start(0, async (request) => {
  return {
    statusCode: 200,
    body: 'Hello World',
  };
});
console.log(`服务器启动在端口: ${actualPort}`);
```

#### `port: number` (Getter)

返回当前正在监听的端口号。如果服务器未运行，返回 `0`。

#### `stop(): Promise<void>`

停止 HTTP 服务器。

**示例**:
```typescript
await server.stop();
```

#### `isRunning(): Promise<boolean>`

检查 HTTP 服务器是否正在运行。通过 TCP 连接探测真实 socket 状态（200ms 超时），而非仅返回内存中的状态标记。

**示例**:
```typescript
const alive = await server.isRunning();
```

#### 自动恢复后台挂起的服务

iOS 系统在锁屏约 30 秒后会挂起 App，导致 HTTP 服务的 TCP socket 被系统关闭。开启 `autoRestart` 后，库内部会自动监听 App 前后台切换，当检测到服务被挂起时自动重启，无需在 App 中添加额外代码。

```typescript
// 方式一：使用 ServerOptions
await server.start(8080, handler, { autoRestart: true });

// 方式二：同时设置 host 和 autoRestart
await server.start(8080, handler, {
  host: '0.0.0.0',
  autoRestart: true,
});

// 方式三：兼容旧版 host 字符串写法（不含 autoRestart）
await server.start(8080, handler, '0.0.0.0');
```

> **工作原理**：进入后台时通过 `beginBackgroundTask` 争取额外存活时间（约 30 秒～3 分钟）；回到前台时通过 TCP connect 真实探测 socket 是否存活——存活则不做任何操作，已死则自动清理并重启。

### StaticServer

静态文件服务器类。

#### `start(port: number, rootDir: string, hostOrOptions?: string | ServerOptions): Promise<number>`

启动静态 file 服务器。

**参数**:
- `port`: 端口号。传入 `0` 为随机端口。
- `rootDir`: 静态文件根目录的绝对路径
- `hostOrOptions`: (可选) 监听的IP地址字符串，或 `ServerOptions` 配置对象

**返回**: 实际监听的端口号。如果启动失败返回 `0`

**示例**:
```typescript
import { StaticServer } from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new StaticServer();
const success = await server.start(
  8080,
  RNFS.DocumentDirectoryPath + '/www',
  '0.0.0.0' // 允许外部访问
);
```

#### `stop(): Promise<void>`

停止静态文件服务器。

#### `isRunning(): Promise<boolean>`

检查静态服务器是否正在运行。通过 TCP 连接探测真实 socket 状态。

### AppServer

应用服务器类（混合模式），同时支持静态文件和动态请求。

#### `start(port: number, rootDir: string, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<number>`

启动应用服务器（混合模式）。服务器会首先尝试在 `rootDir` 中查找对应的静态文件。如果找到且方法为 GET,则直接返回文件内容。否则,将请求转发给 `handler` 处理。

**参数**:
- `port`: 端口号
- `rootDir`: 静态文件根目录
- `handler`: 请求处理器
- `hostOrOptions`: (可选) 监听的IP地址字符串，或 `ServerOptions` 配置对象

#### `stop(): Promise<void>`

停止应用服务器.

#### `isRunning(): Promise<boolean>`

检查应用服务器是否正在运行。通过 TCP 连接探测真实 socket 状态。

### 类型定义

#### HttpRequest

```typescript
interface HttpRequest {
  requestId: string;      // 请求唯一ID
  method: string;         // HTTP 方法 (GET, POST, PUT, DELETE, etc.)
  path: string;           // 请求路径
  headers: Record<string, string>;  // 请求头
  body?: string;          // 请求体（可选）
  binaryBody?: ArrayBuffer; // 二进制请求体（buffer_upload 插件使用）
}
```

#### HttpResponse

```typescript
interface HttpResponse {
  statusCode: number;     // HTTP 状态码 (200, 404, 500, etc.)
  headers?: Record<string, string>;  // 响应头（可选）
  body?: string | ArrayBuffer;       // 响应体（支持 string 或 ArrayBuffer）
}
```

#### `stopAppServer(): Promise<void>`

停止应用服务器。

### ConfigServer

带插件配置支持的服务器类（WebDAV、Zip 挂载等）。

#### `start(port: number, handler: RequestHandler, config: ServerConfig, hostOrOptions?: string | ServerOptions): Promise<number>`

启动带插件配置的服务器。

**参数**:
- `port`: 端口号
- `handler`: 请求处理器
- `config`: 插件配置对象（包含 `root_dir`）
- `hostOrOptions`: (可选) 监听的IP地址字符串，或 `ServerOptions` 配置对象

**示例**:
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

停止配置服务器。

#### `isRunning(): Promise<boolean>`

检查配置服务器是否正在运行。通过 TCP 连接探测真实 socket 状态。

### 帮助函数

#### `createHttpServer(port: number, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<HttpServer>`

创建并启动基础 HTTP 服务器。

#### `createStaticServer(port: number, rootDir: string, hostOrOptions?: string | ServerOptions): Promise<StaticServer>`

创建并启动静态文件服务器。

#### `createAppServer(port: number, rootDir: string, handler: RequestHandler, hostOrOptions?: string | ServerOptions): Promise<AppServer>`

创建并启动应用服务器（混合模式）。

#### `createConfigServer(port: number, handler: RequestHandler, config: ServerConfig, hostOrOptions?: string | ServerOptions): Promise<ConfigServer>`

创建并启动带插件配置的服务器。

### 类型定义

#### HttpRequest

```typescript
interface HttpRequest {
  requestId: string;      // 请求唯一ID
  method: string;         // HTTP 方法 (GET, POST, PUT, DELETE, etc.)
  path: string;           // 请求路径
  headers: Record<string, string>;  // 请求头
  body?: string;          // 请求体（可选）
  binaryBody?: ArrayBuffer; // 二进制请求体（buffer_upload 插件使用）
}
```

#### HttpResponse

```typescript
interface HttpResponse {
  statusCode: number;     // HTTP 状态码 (200, 404, 500, etc.)
  headers?: Record<string, string>;  // 响应头（可选）
  body?: string | ArrayBuffer;       // 响应体（支持 string 或 ArrayBuffer）
}
```

#### ServerOptions

```typescript
interface ServerOptions {
  /** 监听的 IP 地址，默认 127.0.0.1 */
  host?: string
  /**
   * 锁屏/切换App后回到前台时，自动检测并重启被系统挂起的服务。
   * 默认 false，设为 true 后无需在 App 中添加额外代码。
   */
  autoRestart?: boolean
}
```

#### ServerConfig

```typescript
interface ServerConfig {
  root_dir?: string;             // 静态文件根目录（可选，作为默认静态挂载点）
  verbose?: boolean | 'off' | 'error' | 'warn' | 'info' | 'debug'; // 日志等级 (默认 'off')
  mime_types?: MimeTypesConfig;
  mounts?: Mountable[];          // 统一挂载列表
}

type Mountable = WebDavMount | ZipMount | StaticMount | UploadMount | BufferUploadMount | RewriteMount | WebSocketMount;

interface WebDavMount {
  type: 'webdav';
  path: string;      // 挂载点，如 "/webdav"
  root: string;      // WebDAV 根目录
}

interface ZipMount {
  type: 'zip';
  path: string;      // 挂载点，如 "/zip"
  zip_file: string;  // Zip 文件路径
}

interface UploadMount {
  type: 'upload';
  path: string;      // 挂载点，如 "/upload"
  temp_dir: string;  // 上传文件的临时存储目录
}

interface BufferUploadMount {
  type: 'buffer_upload';
  path: string;      // 挂载点，如 "/buffer-upload"
}

interface RewriteMount {
  type: 'rewrite';
  rules: RewriteRule[];
}

interface WebSocketMount {
  type: 'websocket';
  path: string;              // WebSocket 端点，如 "/ws"
  max_message_size?: number; // 最大消息大小（字节，默认 64MB）
}

// WebSocket 连接请求信息
interface WebSocketConnectionRequest {
  path: string;                      // 连接路径
  query: string;                     // 查询字符串
  headers: Record<string, string>;   // HTTP 握手头
}

// WebSocket 连接处理器
type WebSocketConnectionHandler = (
  ws: ServerWebSocket, 
  request: WebSocketConnectionRequest
) => void;
```

```

#### RequestHandler

```typescript
type RequestHandler = (request: HttpRequest) => Promise<HttpResponse> | HttpResponse;
```

请求处理器可以返回 Promise 或直接返回响应对象。

### Node.js 兼容层

导出以下与 Node.js `http` 模块兼容的对象和函数：

- `createServer(requestListener?: (req: IncomingMessage, res: ServerResponse) => void): Server`
- `Server` 类
- `IncomingMessage` 类
- `ServerResponse` 类
- `STATUS_CODES`
- `METHODS`

#### 流式 API

本库还提供了底层的流式 API 用于高级场景：

- `readRequestBodyChunk(requestId: string): Promise<string>` - 分块读取请求体
- `writeResponseChunk(requestId: string, chunk: string): Promise<boolean>` - 分块写入响应体
- `endResponse(requestId: string, statusCode: number, headersJson: string): Promise<boolean>` - 结束流式响应
- `sendBinaryResponse(requestId: string, statusCode: number, headersJson: string, body: ArrayBuffer): Promise<boolean>` - 发送二进制响应

这些 API 在内部被 Node.js 兼容层用于实现流式支持。

## 🏗️ 架构

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

### 技术栈

- **JavaScript 层**: TypeScript, React Native
- **桥接层**: Nitro Modules (C++)
- **核心层**: Rust (Actix-web, Tokio)

### 数据流

1. **请求到达**: Rust Actix-web 服务器接收 HTTP 请求
2. **C 回调**: 通过 FFI 调用 C 回调函数
3. **C++ 转换**: C++ 将 C 结构体转换为 Nitro 类型
4. **JavaScript 调用**: 通过 Nitro Modules 调用 JavaScript 处理器
5. **响应返回**: JavaScript 返回响应 → C++ → C → Rust → HTTP 客户端

## 🔧 常见问题

### Q: 为什么服务器启动失败？

**A**: 可能的原因：
1. **端口被占用**: 尝试更换端口号
2. **权限不足**: 某些端口（如 80, 443）需要 root 权限
3. **防火墙**: 检查防火墙设置

### Q: 如何处理大文件上传？

**A**: 当前版本的 `body` 字段是字符串类型，不适合处理大文件。建议：
- **使用 `UploadPlugin` (推荐)**: 配置 `upload` 挂载点。它会拦截 multipart 上传，将文件保存到临时目录，并将文件路径注入到请求头 (`x-uploaded-file-path`)，避免 JS 处理大字符串。
- **使用 `BufferUploadPlugin`**: 在内存中处理文件（限制 100MB）。通过 `request.binaryBody` 访问数据。
- 使用静态文件服务器 (用于下载)。
- 在 Rust 层添加流式处理支持 (高级)。

### Q: 支持 HTTPS 吗？

**A**: 当前版本不直接支持 HTTPS。建议使用反向代理（如 Nginx）来提供 HTTPS 支持。

### Q: 性能如何？

**A**: 基于 Rust 的 Actix-web 框架，性能非常优秀。以下是基准测试结果（测试环境：MacMini M4, 1 Thread, 2 Connections）：

| 模式 | QPS (Req/Sec) | 延迟 (Latency Avg) |
| :--- | :--- | :--- |
| **基础 HTTP** | **~41.85k** | **~58.14us** |
| **Node.js 兼容 API** | **~21.60k** | **~274.81us** |
| **Koa 框架** | **~13.32k** | **~313.10us** |
| **二进制模式** | **~35.46k** | **~124.29us** |

*注：Node.js 兼容层由于涉及更多的 JavaScript 桥接和对象转换，性能会低于原生 Rust 实现，但仍然足以满足大多数应用场景。*

### Q: 可以同时运行动态服务器和静态服务器吗？



**A**: 可以。你可以分别启动动态服务器和静态服务器（使用不同端口），或者使用 `startAppServer` 在同一个端口上同时提供静态文件和动态 API 服务。

```typescript
// 方法 1: 使用 startAppServer (推荐)
await server.startAppServer(8080, staticDir, apiHandler);

// 方法 2: 分别启动 (不同端口)
await server.start(8080, handler);

// 静态服务器在 8081
await server.startStaticServer(8081, staticDir);
```

### Q: 如何调试服务器问题？

**A**: 
1. 检查服务器日志（Xcode/Logcat）
2. 使用 `getStats()` 查看统计信息
3. 使用工具测试（curl, Postman）

```bash
# 测试服务器
curl http://localhost:8080/api/test
```

## 📝 更新日志

### 1.9.0

- 🔄 `start()` 方法支持 `ServerOptions` 配置对象（向后兼容旧版 `host` 字符串）
- 🔁 新增 `autoRestart` 选项：锁屏后回到前台自动检测并重启被挂起的服务
- 🔍 `isRunning()` 改为异步方法，通过 TCP 连接探测真实 socket 状态
- 🛡️ iOS 后台任务保活：进入后台时自动请求额外执行时间

### 1.8.0 (2026-03-18)

- ✨ 支持通过传入端口号 `0` 来实现 **随机端口分配**。
- 🔄 增强了所有服务器的 `start` 方法，现在返回 **实际监听的端口号** (number) 而非布尔值。
- 🏗️ 为所有服务器类添加了 `port` 属性，用于获取当前运行的端口。
- 🛡️ 提升了 Rust 核心的稳定性，通过优雅处理端口绑定错误并确保在多线程环境下的 Tokio Runtime 兼容性，防止崩溃。

### 1.0.0 (2025-12-08)

- 🎉 初始版本发布
- ✅ 基于 Nitro Modules 的完整实现
- ✅ 支持动态请求处理
- ✅ 支持静态文件服务
- ✅ iOS 和 Android 支持

## 📄 许可证

ISC

## 🤝 贡献

欢迎提交 Issue 和 Pull Request！

## 🔗 相关链接

- [Nitro Modules](https://github.com/mrousavy/nitro)
- [Actix-web](https://actix.rs/)
- [React Native](https://reactnative.dev/)

---

**Made with ❤️ using Rust, C++, and React Native**
