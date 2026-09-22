# File Upload Plugin

Upload 插件用于处理 `multipart/form-data` 格式的文件上传请求。它会自动将上传的文件保存到服务器指定的临时目录中，并通过 HTTP Header 将文件保存路径传递给后续的处理逻辑。

## 功能特性

- **自动文件保存**：无需手动解析流，自动将 multipart 文件流保存为磁盘文件。
- **UUID 命名**：使用 UUID 生成唯一文件名，防止重名覆盖。
- **Header 传递**：通过 `X-Uploaded-File-Path` 等头部字段将文件信息传递给业务层。
- **流式处理**：高效处理大文件上传，低内存占用。

## 配置

在服务器配置的 `mounts` 数组中添加类型为 `upload` 的项。

```typescript
// 类型定义
interface UploadMount {
  type: 'upload';
  path: string;           // 监听上传的路径 (POST/PUT)
  temp_dir: string;       // 文件保存的临时目录路径
}
```

### 完整配置示例

```json
{
  "mounts": [
    {
      "type": "upload",
      "path": "/api/upload",
      "temp_dir": "./temp_uploads"
    }
  ]
}
```

## 工作原理

1. 插件拦截匹配 `path` 且方法为 `POST` 或 `PUT` 的请求。
2. 检查 `Content-Type` 是否为 `multipart/form-data`。
3. 解析 multipart 数据流，将文件部分写入 `temp_dir` 下的新文件（文件名格式：`{uuid}.{ext}`）。
4. 文件保存完成后，插件会**重构请求**，将其传递给下一个处理器（PassThrough）。
5. 重构后的请求包含以下自定义 Header，供后续业务逻辑使用：
    - `X-Uploaded-File-Path`: 文件在服务器上的绝对路径。
    - `X-Uploaded-Original-Name`: 原始上传文件名。
    - `X-Uploaded-Size`: 文件大小（字节）。

## 注意事项

- **请求体变空**：由于 multipart 流已被插件消费，传递给后续 JS 层的 Request Body 将为空。请直接读取 Header 获取文件信息。
- **临时文件清理**：插件只负责保存文件。业务逻辑处理完（如移动到永久存储或处理图片）后，应负责清理该临时文件，或配置系统的定期清理任务。
- **覆盖范围**：目前插件主要处理第一个上传的文件。
