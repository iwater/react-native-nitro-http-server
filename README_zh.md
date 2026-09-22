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
- ⛔ **感知客户端断开**: `request.signal` / `req.aborted` / `res.destroyed`，可据此取消下游操作
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

### CORS 与挂载点自定义响应头

为服务器启用 CORS，并为挂载的插件自定义响应头。

```typescript
import { createConfigServer, setCorsConfig } from 'react-native-nitro-http-server';

// 方式一：config server 的 cors 字段
await createConfigServer(port, handler, {
  cors: true, // 或 { origin: 'https://app.com', credentials: true, max_age: 600 }
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

// 方式二：其他 server 类型启动前调用 setCorsConfig
setCorsConfig(true);
await createStaticServer(port, rootDir);
```

**注意**：
- `setCorsConfig` / `config.cors` 应在 server 启动前设置。
- `cors: true` 的默认行为：返回 `Access-Control-Allow-Origin: *`，并自动处理 preflight（OPTIONS）请求。
- 当 `credentials: true` 时，`Access-Control-Allow-Origin` 回显请求的 `Origin`。
- 挂载点上的 `headers` 字段允许覆盖插件默认头；对 rewrite/websocket/upload mount 为无操作。

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

### 感知客户端断开（`req.aborted` / `request.signal`）

客户端在 handler 返回响应之前断开（关页面 / 取消请求 / 网络断了）时，此前 JS 侧
**完全收不到通知** —— 只能在发送响应失败或等到 `request_timeout_secs` 超时时才间接察觉。
现在两条路径都有信号：

```typescript
// ① 高层 handler：用 signal（推荐 —— 能直接喂给 fetch / 下游 SDK 的取消参数）
import { HttpServer } from 'react-native-nitro-http-server';

const server = new HttpServer();
await server.start(8080, async (req) => {
    try {
        // 客户端断开时这个 fetch 会被取消，不必等它自己超时
        const r = await fetch('https://upstream.example/api', { signal: req.signal });
        return { statusCode: 200, body: await r.text() };
    } catch (e) {
        if (req.signal?.aborted) return { statusCode: 499, body: 'client closed request' };
        throw e;
    }
});

// ② Node 兼容层：req.aborted / 'aborted' 事件 / res.destroyed
import { createServer } from 'react-native-nitro-http-server';

const nodeServer = createServer((req, res) => {
    req.on('aborted', () => console.log('客户端走了'));
    longQuery().then((data) => {
        if (req.aborted) return;          // 结果已经没人要了
        res.end(data);
    });
});
```

与 Node 的对齐（下表逐项是 Node 22 实测值）：

| 信号 | 本模块 | Node 22 |
| :--- | :--- | :--- |
| `req.aborted` | ✅ | ✅ |
| `req.on('aborted')` | ✅ | ✅（v17 起已废弃） |
| `req.destroyed` / `res.destroyed` | ✅ | ✅ |
| `res.on('close')` | ✅ | ✅ |
| `req.on('error')`（`code === 'ECONNRESET'`） | ✅ 仅在**已有** `'error'` 监听器时发 | ✅ 同样只在有监听器时发 |
| `request.signal` | ✅ **本模块扩展** | ❌ `http.IncomingMessage` 上没有这个属性 |

⚠️ 触发时机**只在「响应从未发出」时**：正常回完、handler 抛异常、请求超时都**不会**触发。

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

// 基础用法
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

**启用 `autoRestart`：**

```typescript
// 将 ServerOptions 作为第一个参数传入
const server = createServer({ autoRestart: true }, (req, res) => {
  res.end('Hello');
}).listen(3000);

// 或直接使用 Server 类
import { Server } from 'react-native-nitro-http-server';
const server = new Server({ autoRestart: true });
server.on('request', (req, res) => res.end('Hello'));
server.listen(3000);
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

#### `setCorsConfig(config: boolean | CorsConfig): void`

设置全局 CORS 配置，对所有 server 类型生效（`HttpServer`、`StaticServer`、`AppServer`、Node.js 兼容的 `http.createServer`）。应在 server 启动前调用。`true` 启用默认配置（`Access-Control-Allow-Origin: *`，自动处理 preflight 请求），对象自定义行为，`false` 关闭 CORS。

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
  cors?: boolean | CorsConfig;   // CORS 配置：true = 默认（Allow-Origin: *），对象自定义，缺省关闭
  mime_types?: MimeTypesConfig;
  mounts?: Mountable[];          // 统一挂载列表
  /**
   * 等 JS 回调返回响应的最长时间（秒），默认 30。
   * 长轮询 / SSE 必须调大，否则连接会被硬切断并返回 500。
   *
   * ⚠️ **粘性全局**（见「已知限制」）：只在**传了本字段**时才写入 ——
   * 于是先前某个 server 设过的值会**继续作用于之后不传该字段的 server**。
   * 想复位就显式传 30。
   */
  request_timeout_secs?: number
  /**
   * 回调路径的请求体上限（字节），默认 100MB。超限返回 **413** 且**不回调**你的 handler。
   * 插件路径（static / zip / upload / webdav）不读 body，不受这个上限约束。
   *
   * ⚠️ **粘性全局**（见「已知限制」）：与 `request_timeout_secs` 同理。
   * 想复位就显式传 `100 * 1024 * 1024`。
   */
  max_body_size?: number
}

interface CorsConfig {
  origin?: string;        // 默认 "*"
  methods?: string[];     // 默认：常见方法 + WebDAV 方法
  headers?: string[];     // 默认 ["*"]；preflight 时优先回显请求的 Access-Control-Request-Headers
  credentials?: boolean;  // 默认 false；为 true 时 Allow-Origin 回显请求 Origin
  max_age?: number;       // preflight 缓存秒数
}

type Mountable = WebDavMount | ZipMount | StaticMount | UploadMount | BufferUploadMount | RewriteMount | WebSocketMount;

// 除 RewriteMount 外，所有挂载点还支持：
headers?: Record<string, string>; // 自定义响应头，可覆盖插件默认头（对 rewrite/websocket/upload mount 为无操作）

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

- `createServer(options?: ServerOptions, requestListener?: (req: IncomingMessage, res: ServerResponse) => void): Server`
- `createServer(requestListener?: (req: IncomingMessage, res: ServerResponse) => void): Server`
- `Server` 类 — 构造函数 `new Server(options?: ServerOptions, requestListener?)`
- `IncomingMessage` 类
- `ServerResponse` 类
- `STATUS_CODES`
- `METHODS`

#### `ServerOptions` (Node.js 兼容层)

```typescript
interface ServerOptions {
  IncomingMessage?: typeof IncomingMessage;
  ServerResponse?: typeof ServerResponse;
  requestTimeout?: number;      // 默认 300000
  headersTimeout?: number;      // 默认 60000
  keepAliveTimeout?: number;    // 默认 5000
  /** 锁屏后回到前台自动检测并重启被挂起的服务。默认 false */
  autoRestart?: boolean;
}
```

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

## ⚠️ 已知限制

有意为之的行为、尚未实现的功能、以及平台约束。每条都写明**你会观察到什么**与**该怎么绕**。

### `server.close()` 是异步的（stop→start 的顺序模块已替你排好）

`close()` 只是**发起**原生 stop 就返回；`'close'` 事件与 `close(cb)` 回调要等
**原生侧真的停完**才触发。

由于原生 server 是**单例**，落在这个窗口里的 `start()` 过去会与那次拆卸撞车：
新 server 正常报出端口，但**它的请求完全拿不到响应**（客户端看到连接/读取失败，
不是 500）—— 而且是**偶发**，看起来就像"测试抖动"。

**2026-09-22 已修** —— `start()` 现在会先等所有在飞的原生 stop 落定再继续
（`src/nativeStopQueue.ts`），于是 Node 那种写法重新变安全：

```typescript
oldServer.close();
newServer.listen(8080);   // ✓ 安全了 —— start() 会等在飞的 stop
```

真机已验证（真机 RN 套件的 D11 就是**故意**这么写的）。
你自己的代码里若想要显式的顺序保证，仍然可以 `await new Promise(r => old.close(() => r()))`，
没有代价。

### `request.signal` 需要宿主提供 `AbortController`

`request.signal`（以及它背后的 `AbortController`）**只在宿主实现了它时才存在**：

- **React Native 有** —— `Libraries/Core/setUpXHR.js` 里有
  `polyfillGlobal('AbortController', …)`，所以真机上正常可用；
- **无头 JS 宿主一般没有** —— 本模块做端到端验证的那个无头宿主实测
  `typeof globalThis.AbortController === 'undefined'`。

那种环境下 `req.signal` 就是 `undefined`，**其余中断信号照常工作**
（`req.aborted` / `'aborted'` / `res.destroyed` 都不依赖 `AbortController`）。

→ 用之前判一下：`if (req.signal) { ... }`，或者用可选链 `req.signal?.aborted`。
判据是 `typeof AbortController === 'undefined'`。

模块刻意**不做**内部兜底实现：一个只在测试宿主上跑得起来的自制 signal，
会让用例验的不是真东西。

### 多个 callback 型服务器共享同一个原生 handler

`HttpServer` / `AppServer` / `ConfigServer` 都把 handler 注册进**同一个全局槽**。
启动第二个会**覆盖**第一个，于是**任意端口**上的请求都被路由到**最后启动**的那个 handler。

```typescript
await serverA.start(8080, handlerA);   // 此时 handlerA 生效
await serverB.start(8081, handlerB);   // 现在两个端口都调 handlerB
```

- **正确做法**：只跑一个 callback 型服务器，用 `ConfigServer` 的 mounts
  （`static` / `zip` / `upload` / `rewrite` / `websocket`）分流。
- 纯静态服务器不受影响 —— `StaticServer` 从不使用那个全局槽。

### Node 兼容层：超时属性不生效

`server.requestTimeout` / `headersTimeout` / `keepAliveTimeout`（以及 `server.timeout`）
只是 Node 兼容层 `Server` 上的普通属性：**可读可写，但没有任何代码读它们**，改了没有效果。

要真正控制超时，请用 config server 的 `request_timeout_secs`（秒，默认 30）。

### Node 兼容层：请求/响应体走 UTF-8 字符串通道

`HttpRequest.body` 与 `HttpResponse.body` 都是 `string`，任何非 UTF-8 字节序列在进出时都会
被损坏 —— **不要**用它们传二进制。

两个逃生口：

| 方向 | 做法 |
| :--- | :--- |
| 请求 | `request.binaryBody`（`ArrayBuffer`）—— **仅** `buffer_upload` 透传的请求（带 `x-upload-filename` 头）才会带上 |
| 响应 | Node 兼容层用 `res.end(arrayBuffer)`，或让 `createHttpServer` 的 handler 返回 `ArrayBuffer` body |

⚠️ 已知缺口：`res.write('a'); res.end(arrayBuffer)` 里的 `'a'` 会被**静默丢弃** ——
二进制路径发送的是显式 body、不读累积器；串行化只保证顺序。修它需要 Rust 侧改动。

### 上传插件：文件名 header 在非 ASCII 时是 percent 编码

两个上传插件用的 **header 名不同**：

| 插件 | 文件名 header | 标记 header |
| :--- | :--- | :--- |
| `buffer_upload` | `X-Upload-Filename` | `X-Upload-Filename-Encoding: percent` |
| `upload` | `X-Uploaded-Original-Name` | `X-Uploaded-Filename-Encoding: percent` |

值**只在文件名不能被 header 承载时**（非 ASCII）才 percent 编码，而标记 header
**也只在这种情况下出现**。所以要先看标记再解码 —— 无脑 `decodeURIComponent`
会把本来合法含 `%` 的文件名改坏：

```typescript
const name = request.headers['x-uploaded-original-name'];
const filename = request.headers['x-uploaded-filename-encoding'] === 'percent'
  ? decodeURIComponent(name)
  : name;
```

### `upload` 挂载：临时文件从不清理，且只报第一个文件

- 文件写进 `temp_dir` 之后就**留在那里** —— 插件从不删除它们。请把 `temp_dir` 指向
  系统会自动回收的位置（如 caches 目录），并自行清理。
- 一个 `multipart` 请求带多个文件时会被接受，但只有**第一个**文件的信息出现在
  header 里（`x-uploaded-file-path` 等）。需要全部的话请自行遍历请求。

### `request_timeout_secs` / `max_body_size` 是**粘性全局**

两者都存在原生全局里，而且**只在传了对应字段时**才被写入（`global.rs` / `app_server.rs`）。
`stop()` 不会复位它们，所以某个 server 设过的值会**继续作用于之后每一个 server** ——
哪怕后来那个完全不传这两个字段：

```typescript
await cfgA.start(0, h, { max_body_size: 1024 });   // 1 KB 上限
await cfgA.stop();
await HttpServerModule.start(0, h);                // 完全不传 config —— 仍是 1 KB！
// → 这时一个 1 MB 的 POST 会拿到 413
```

很容易踩：一个页面为了做上传测试把 `max_body_size` 设得很小，另一个页面以为还是默认值。
**只要你的 app 会起多个 server，每次都把想要的值显式传上**（或显式复位）。

> 这条是真机 RN 套件跑出来的 —— mounts 套件设过
> `max_body_size: 1024` 之后，「核心」套件里的 1 MB POST 就开始返 413。

### `zip` 挂载会把整个压缩包放进内存

zip 挂载启动时读一次、常驻内存（之后请求零文件 I/O）。适合资源包，
**不适合** 100MB 以上的压缩包。

### WebSocket `send()` 可能返回 `false`

`ws.send()` 返回 `false` 表示消息**没有入队**：连接已不在，或该连接的发送队列
（256 条）已满。消息不会排队、也不会自动重试 —— 由调用方决定：

```typescript
if (!(await ws.send(data))) {
  // 退避重试，或关掉这条连接
}
```

### `getStats()` —— 6 个字段里只有 3 个是真实的

| 字段 | 状态 |
| :--- | :--- |
| `totalRequests` | **真实** |
| `errorCount` | **真实** —— 只统计服务器自身故障（回调超时、响应通道被关闭、静态插件 I/O 失败）。你自己的 handler 返回的 4xx/5xx **不计入** |
| `uptime` | **真实** —— 距最近一次成功启动的秒数 |
| `activeConnections` | 恒 `0` —— hyper 0.14 的 `Server::serve` 不暴露每连接钩子 |
| `bytesSent` / `bytesReceived` | 恒 `0` —— 响应出口不止一个，只统计其中一部分会比报 `0` 更糟 |

### 静态文件服务不支持 Range / 条件请求

`Range`、`ETag`、`If-Modified-Since` 都未实现 —— 每个响应都是完整的 `200`。
需要的话在前面挂 CDN 或反向代理。

### 多个 WebSocket mount 只有最后一个可达

WebSocket 插件是全局单例，配置多个 `websocket` mount 时只有最后一个能收到升级请求。

### 不要混用 `onWebSocket()` 与独立的 `setupWebSocketHandler()`

两者装进的是**同一个全局单回调**（后装者胜），而 `ConfigServer` 在自动重启时会重挂处理器。
请二选一：

- `server.onWebSocket('/ws', handler)` —— 按路径分发（推荐）
- `setupWebSocketHandler(handler)` —— 一个 handler 处理全部

真混用了的话：自动重启会保留**重启前最后安装**的那一个，不会再悄悄换主。

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

> ⚠️ 这能成立是因为 `StaticServer` 从不使用那个全局回调槽。但**两个 callback 型服务器**
> （`HttpServer` / `AppServer` / `ConfigServer`）是另一回事 —— 它们共享同一个全局 handler，
> 后启动的胜出。见[已知限制](#多个-callback-型服务器共享同一个原生-handler)。

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

### Unreleased

- **新增：客户端断开时通知 JS。** 客户端在 handler 返回响应之前断开时，此前 JS 侧收不到
  任何信号 —— 只能靠「发送响应失败」或等 `request_timeout_secs` 超时来间接察觉。现在有
  两条路径：高层 handler 拿 `request.signal`（可直接喂给 `fetch` / 下游 SDK 的取消参数），
  Node 兼容层拿 `req.aborted` / `'aborted'` 事件 / `res.destroyed`。
  除 `request.signal`（本模块扩展，Node 的 `IncomingMessage` 没有）外逐项与 **Node 22 实测**
  对齐。**只在「响应从未发出」时触发** —— 正常回完 / handler 抛异常 / 请求超时都不触发。
  跨四层实现（Rust 判定 → C 头 → C++ 回调 → Nitro 切线程 → TS）。
- **修复 `close()` → `start()` 的竞态**：原生 server 是**单例**，而 `close()` 只**发起**
  原生 stop（`'close'` 回调要等原生停完才触发）。落在这个窗口里的 `start()` 会与那次拆卸
  撞车 —— 新 server 正常报出端口，但**请求完全拿不到响应**（客户端看到连接/读取失败，
  不是 500），且**偶发**，看起来就像测试抖动。现在 `start()` 会先等所有在飞的原生 stop
  落定（`src/nativeStopQueue.ts`）。真机已验证（真机 RN 套件 D11）。
- **文档修正**：`request_timeout_secs` / `max_body_size` 标注为**粘性全局** —— 它们只在
  **传了字段**时才写入，`stop()` 不复位，所以某个 server 设过的值会继续作用于之后
  不传该字段的 server。真机套件跑出来的（一轮里 1MB POST 突然变 413）。

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
