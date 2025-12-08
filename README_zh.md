# React Native HTTP Server

一个高性能的 React Native HTTP 服务器库，基于 Rust 实现，支持动态请求处理和静态文件服务。

## ✨ 特性

- 🚀 **高性能**: 基于 Rust 的 Actix-web 框架，性能卓越
- 📱 **跨平台**: 支持 iOS 和 Android
- 🔄 **异步处理**: 使用 Nitro Modules 提供原生异步 API
- 📁 **静态文件服务**: 内置静态文件服务器支持
- 🎯 **简单易用**: TypeScript 友好的 API 设计
- ⚡ **零拷贝**: 直接通过 FFI 调用 Rust 代码

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
import ReactNativeHttpServer from 'react-native-nitro-http-server';

const server = new ReactNativeHttpServer();

// 启动服务器
await server.start(8080, async (request) => {
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

console.log('服务器运行在 http://localhost:8080');

// 停止服务器
// await server.stop();
```

### 静态文件服务器

```typescript
import ReactNativeHttpServer from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new ReactNativeHttpServer();

// 启动静态文件服务器
const staticDir = RNFS.DocumentDirectoryPath + '/www';
await server.startStaticServer(8080, staticDir);

console.log(`静态文件服务器运行在 http://localhost:8080`);
console.log(`服务目录: ${staticDir}`);

// 停止静态服务器
// await server.stopStaticServer();
```

### 应用服务器 (混合模式)

同时支持静态文件服务和动态 API 处理。优先尝试服务静态文件，如果文件不存在则调用回调函数。

```typescript
import ReactNativeHttpServer from 'react-native-nitro-http-server';
import RNFS from 'react-native-fs';

const server = new ReactNativeHttpServer();
const staticDir = RNFS.DocumentDirectoryPath + '/www';

// 启动应用服务器（混合模式）
await server.startAppServer(8080, staticDir, async (request) => {
  // 静态文件不存在时会执行此回调
  return {
    statusCode: 200,
    body: `Dynamic response for ${request.path}`,
  };
});
```

### RESTful API 示例

```typescript
import ReactNativeHttpServer from 'react-native-nitro-http-server';

const server = new ReactNativeHttpServer();

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

### ReactNativeHttpServer

主要的服务器类。

#### `start(port: number, handler: RequestHandler, host?: string): Promise<boolean>`

启动 HTTP 服务器。

**参数**:
- `port`: 端口号（1024-65535）
- `handler`: 请求处理函数,接收 `HttpRequest` 并返回 `HttpResponse`
- `host`: (可选) 监听的IP地址,默认为 `127.0.0.1`。传入 `0.0.0.0` 可允许外部访问

**返回**: 如果启动成功返回 `true`

**示例**:
```typescript
// 仅本地访问(默认)
const success = await server.start(8080, async (request) => {
  return {
    statusCode: 200,
    body: 'Hello World',
  };
});

// 允许外部访问
await server.start(8080, handler, '0.0.0.0');
```

#### `stop(): Promise<void>`

停止 HTTP 服务器。

**示例**:
```typescript
await server.stop();
```

#### `startStaticServer(port: number, rootDir: string, host?: string): Promise<boolean>`

启动静态文件服务器。

**参数**:
- `port`: 端口号
- `rootDir`: 静态文件根目录的绝对路径
- `host`: (可选) 监听的IP地址,默认为 `127.0.0.1`。传入 `0.0.0.0` 可允许外部访问

**返回**: 如果启动成功返回 `true`

**示例**:
```typescript
import RNFS from 'react-native-fs';

const success = await server.startStaticServer(
  8080,
  RNFS.DocumentDirectoryPath + '/www',
  '0.0.0.0' // 允许外部访问
);
```

#### `stopStaticServer(): Promise<void>`

停止静态文件服务器。

#### `getStats(): Promise<ServerStats>`

获取服务器统计信息。

**返回**: 包含以下字段的对象：
- `totalRequests`: 总请求数
- `activeConnections`: 活动连接数
- `bytesSent`: 发送的字节数
- `bytesReceived`: 接收的字节数
- `uptime`: 运行时间（秒）
- `errorCount`: 错误计数

#### `isServerRunning(): boolean`

检查动态服务器是否正在运行。

#### `isStaticRunning(): boolean`

检查静态服务器是否正在运行。

#### `startAppServer(port: number, rootDir: string, handler: RequestHandler, host?: string): Promise<boolean>`

启动应用服务器（混合模式）。服务器会首先尝试在 `rootDir` 中查找对应的静态文件。如果找到且方法为 GET,则直接返回文件内容。否则,将请求转发给 `handler` 处理。

**参数**:
- `port`: 端口号
- `rootDir`: 静态文件根目录
- `handler`: 请求处理器
- `host`: (可选) 监听的IP地址,默认为 `127.0.0.1`

#### `stopAppServer(): Promise<void>`

停止应用服务器。

#### `static createStaticServer(port: number, staticDir: string): Promise<ReactNativeHttpServer>`

静态便捷方法，创建并启动一个静态文件服务器实例。

### 类型定义

#### HttpRequest

```typescript
interface HttpRequest {
  requestId: string;      // 请求唯一ID
  method: string;         // HTTP 方法 (GET, POST, PUT, DELETE, etc.)
  path: string;           // 请求路径
  headers: Record<string, string>;  // 请求头
  body?: string;          // 请求体（可选）
}
```

#### HttpResponse

```typescript
interface HttpResponse {
  statusCode: number;     // HTTP 状态码 (200, 404, 500, etc.)
  headers?: Record<string, string>;  // 响应头（可选）
  body?: string;          // 响应体（可选）
}
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

注意：由于 React Native 环境限制，流式 API 目前可能以全量数据缓冲形式实现。

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
- 使用静态文件服务器
- 在 Rust 层添加流式处理支持

### Q: 支持 HTTPS 吗？

**A**: 当前版本不直接支持 HTTPS。建议使用反向代理（如 Nginx）来提供 HTTPS 支持。

### Q: 性能如何？

**A**: 基于 Rust 的 Actix-web 框架，性能非常优秀。以下是基准测试结果（测试环境：MacMini M4, 1 Thread, 2 Connections）：

| 模式 | QPS (Req/Sec) | 延迟 (Latency Avg) | 传输速率 (Transfer/Sec) |
| :--- | :--- | :--- | :--- |
| **基础 HTTP 服务器**<br>(ReactNativeHttpServer.start) | **~33,267** | **~83.45us** | **~4.86MB** |
| **Node.js 兼容层**<br>(Koa via createServer) | **~4,421** | **~2.76ms** | **~682.22KB** |

*注：Node.js 兼容层由于涉及更多的 JavaScript 桥接和对象转换，性能会低于原生 Rust 实现，但仍然足以满足大多数应用场景。*

### Q: 可以同时运行动态服务器和静态服务器吗？

**A**: 可以，但它们必须使用不同的端口：

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

### 1.0.0 (2025-12-08)

- 🎉 初始版本发布
- ✅ 基于 Nitro Modules 的完整实现
- ✅ 支持动态请求处理
- ✅ 支持静态文件服务
- ✅ iOS 和 Android 支持

## 📄 许可证

MIT

## 🤝 贡献

欢迎提交 Issue 和 Pull Request！

## 🔗 相关链接

- [Nitro Modules](https://github.com/mrousavy/nitro)
- [Actix-web](https://actix.rs/)
- [React Native](https://reactnative.dev/)

---

**Made with ❤️ using Rust, C++, and React Native**
