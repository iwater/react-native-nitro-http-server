# WebSocket 插件

WebSocket 插件为 HTTP Server 添加 WebSocket 协议支持，允许服务器与客户端建立全双工通信连接。

## 配置

```json
{
  "mounts": [
    {
      "type": "websocket",
      "path": "/ws",
      "max_message_size": 67108864
    }
  ]
}
```

| 参数 | 类型 | 必填 | 默认值 | 描述 |
|------|------|------|--------|------|
| `type` | string | ✅ | - | 必须为 `"websocket"` |
| `path` | string | ✅ | - | WebSocket 端点路径 |
| `max_message_size` | number | ❌ | 67108864 (64MB) | 最大消息大小（字节） |

## 使用示例

### 基本用法

```typescript
import { ConfigServer, WebSocketConnectionHandler } from 'react-native-nitro-http-server';

const server = new ConfigServer();

// 1. 注册 WebSocket 处理器（必须在 start 之前）
server.onWebSocket('/ws', (ws, request) => {
    // request 包含握手信息
    console.log('Path:', request.path);
    console.log('Query:', request.query);     // 如 "token=abc&user=123"
    console.log('Headers:', request.headers); // 完整的 HTTP 握手头
    
    // 设置事件监听器
    ws.onmessage = (e) => {
        console.log('Received:', e.data);
        ws.send('Echo: ' + e.data);
    };
    
    ws.onclose = (e) => {
        console.log('Closed:', e.code, e.reason);
    };
    
    ws.onerror = (e) => {
        console.error('Error:', e.message);
    };
});

// 2. 启动服务器
await server.start(8080, httpHandler, {
    mounts: [
        { type: 'websocket', path: '/ws' }
    ]
});
```

### 多路径支持

```typescript
const server = new ConfigServer();

// 聊天 WebSocket
server.onWebSocket('/chat', (ws, request) => {
    ws.onmessage = (e) => { /* 处理聊天消息 */ };
});

// 通知 WebSocket
server.onWebSocket('/notifications', (ws, request) => {
    ws.onmessage = (e) => { /* 处理通知 */ };
});

// 通配符处理器（匹配所有未注册的路径）
server.onWebSocket('*', (ws, request) => {
    console.log('Unknown path:', request.path);
    ws.close(1008, 'Unknown path');
});

await server.start(8080, httpHandler, {
    mounts: [
        { type: 'websocket', path: '/chat' },
        { type: 'websocket', path: '/notifications' }
    ]
});
```

### 鉴权示例

```typescript
server.onWebSocket('/ws', (ws, request) => {
    // 从 query 或 headers 获取 token
    const token = new URLSearchParams(request.query).get('token');
    const authHeader = request.headers['authorization'];
    
    if (!token && !authHeader) {
        ws.close(1008, 'Authentication required');
        return;
    }
    
    // 验证 token...
    
    ws.onmessage = (e) => {
        ws.send('Authenticated: ' + e.data);
    };
});
```

## TypeScript 类型

```typescript
// WebSocket 挂载配置
interface WebSocketMount {
    type: 'websocket'
    path: string
    max_message_size?: number
}

// 连接请求信息
interface WebSocketConnectionRequest {
    path: string                      // 连接路径
    query: string                     // 查询字符串
    headers: Record<string, string>   // HTTP 握手头
}

// 连接处理器
type WebSocketConnectionHandler = (
    ws: ServerWebSocket, 
    request: WebSocketConnectionRequest
) => void
```

## ServerWebSocket API

```typescript
interface ServerWebSocket {
    readonly connectionId: string      // 连接 ID
    readonly readyState: number        // 0=CONNECTING, 1=OPEN, 2=CLOSING, 3=CLOSED
    
    // 发送消息
    send(data: string | ArrayBuffer): Promise<boolean>
    
    // 关闭连接
    close(code?: number, reason?: string): Promise<boolean>
    
    // 事件处理器
    onopen: (() => void) | null
    onmessage: ((event: { data: string | ArrayBuffer }) => void) | null
    onclose: ((event: { code: number; reason: string; wasClean: boolean }) => void) | null
    onerror: ((event: { message: string }) => void) | null
}
```

## 客户端示例

```javascript
// 浏览器或其他 WebSocket 客户端
const ws = new WebSocket('ws://localhost:8080/ws?token=abc123');

ws.onopen = () => {
    console.log('Connected');
    ws.send('Hello Server!');
};

ws.onmessage = (event) => {
    console.log('Received:', event.data);
};

ws.onclose = (event) => {
    console.log('Closed:', event.code, event.reason);
};
```

## 注意事项

1. **注册顺序**：必须在 `server.start()` 之前调用 `server.onWebSocket()`
2. **路径精确匹配**：处理器按精确路径匹配，使用 `'*'` 作为通配符
3. **消息大小**：默认最大消息大小为 64MB，可通过配置调整
4. **关闭代码**：使用标准 WebSocket 关闭代码（1000 = 正常关闭, 1008 = 策略违规）
