# WebDAV Plugin

WebDAV 插件为服务器提供标准的 WebDAV 协议支持，允许客户端（如 Finder, Windows Explorer, Cyberduck）像操作本地文件一样挂载和管理服务器上的文件。

## 功能特性

- **标准协议支持**：支持 PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK 等标准 WebDAV 方法。
- **文件管理**：支持文件的上传、下载、删除、重命名和移动。
- **目录管理**：支持创建和删除文件夹。
- **虚拟挂载**：将任意本地目录挂载为 WebDAV 服务端点。

## 配置

在服务器配置的 `mounts` 数组中添加类型为 `webdav` 的项。

```typescript
// 类型定义
interface WebDavMount {
  type: 'webdav';
  path: string;           // WebDAV 服务挂载路径 (URL前缀)
  root: string;           // 本地文件系统根目录
}
```

### 完整配置示例

```json
{
  "mounts": [
    {
      "type": "webdav",
      "path": "/webdav",
      "root": "./documents"
    }
  ]
}
```

## 使用说明

### 连接 WebDAV 服务

1. **Mac (Finder)**:
    - 打开 Finder，点击菜单栏 "前往" -> "连接服务器" (Cmd+K)。
    - 输入地址: `http://<device-ip>:<port>/webdav`
    - 点击连接，以访客身份登录。

2. **Windows**:
    - 打开文件资源管理器，右键 "此电脑" -> "映射网络驱动器"。
    - 在文件夹框中输入: `http://<device-ip>:<port>/webdav`

3. **第三方客户端**:
    - 在应用中输入服务器 URL、用户名和密码（目前服务未强制鉴权）。

## 注意事项

- **网络权限**：确保 iOS/Android 设备与客户端处于同一局域网内。
- **并发写入**：虽然支持 LOCK 协议，但在高并发场景下请注意数据一致性。
- **性能**：对于生成缩略图等操作，Finder 可能会发送大量请求，建议在该目录主要用于文件传输而非浏览大量图片。
