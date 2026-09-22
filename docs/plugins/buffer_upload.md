# Buffer Upload Plugin

Buffer Upload 插件用于处理 `multipart/form-data` 格式的文件上传请求，但与 Upload 插件不同，它将文件直接**读取到内存**中，并作为 Request Body 传递给 JavaScript 层。这适用于处理较小的文件（如头像、配置文本），希望在 JS 端直接获取 `ArrayBuffer` 的场景。

## 功能特性

- **内存处理**：文件直接读入内存，不产生临时文件。
- **二进制传递**：将文件内容作为二进制 Body 传递给 JS，方便直接处理。
- **大小限制**：内置 100MB 大小限制，防止内存溢出。
- **元数据头**：通过 `X-Upload-*` 头部传递文件名和类型信息。

## 配置

在服务器配置的 `mounts` 数组中添加类型为 `buffer_upload` 的项。

```typescript
// 类型定义
interface BufferUploadMount {
  type: 'buffer_upload';
  path: string;           // 监听上传的路径
}
```

### 完整配置示例

```json
{
  "mounts": [
    {
      "type": "buffer_upload",
      "path": "/api/upload-memory"
    }
  ]
}
```

## 工作原理

1. 插件拦截匹配 `path` 且方法为 `POST` 或 `PUT` 的请求。
2. 检查 `Content-Type` 是否为 `multipart/form-data`。
3. 解析并读取**第一个**上传的文件到内存缓冲区（最大 100MB）。
4. 插件**重构请求**，将其传递给下一个处理器（PassThrough）：
    - **Header**:
        - `X-Upload-Filename`: 文件名。
        - `X-Upload-Size`: 文件大小。
        - `X-Upload-Content-Type`: 文件 MIME 类型。
    - **Body**: 文件的原始二进制数据 (Binary Data)。

在 JavaScript 端，你可以直接从 `request.body` (ArrayBuffer) 中获取文件内容。

## 对比：Upload vs Buffer Upload

| 特性 | Upload Plugin | Buffer Upload Plugin |
|------|--------------|----------------------|
| **存储方式** | 磁盘临时文件 | 内存缓冲区 |
| **JS 获取内容** | 读取文件路径 (路径在 Header 中) | 直接读取 Body (ArrayBuffer) |
| **适用场景** | 大文件、视频、需要持久化的文件 | 小文件、头像、文本配置、即时处理 |
| **性能** | 本地 IO，内存占用低 | 内存占用高，速度快 |
| **临时文件清理** | 需要业务层手动清理 | 自动回收 (GC) |
