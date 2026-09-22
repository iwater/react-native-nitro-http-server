# Zip Mount Plugin

Zip Mount 插件允许服务器**直接读取并提供 ZIP 压缩包内的文件**，而无需解压。这对于发布游戏资源、H5 应用包或离线内容非常有用，可以显著减少小文件的 IO 开销并简化资源分发。

## 功能特性

- **直接读取**：无需解压，实时从 ZIP 包中流式读取文件内容。
- **透明访问**：客户端对 ZIP 包无感知，访问方式与普通静态文件完全一致。
- **内存优化**：使用流式读取，不会将整个 ZIP 包加载到内存中。
- **MIME 支持**：自动识别包内文件的 MIME 类型，支持自定义 MIME 配置。

## 配置

在服务器配置的 `mounts` 数组中添加类型为 `zip` 的项。

```typescript
// 类型定义
interface ZipMount {
  type: 'zip';
  path: string;           // 挂载的 URL 路径
  zip_file: string;       // 本地 ZIP 文件路径
}
```

### 完整配置示例

```json
{
  "mounts": [
    {
      "type": "zip",
      "path": "/game/assets",
      "zip_file": "./packages/level1_assets.zip"
    }
  ]
}
```

## 工作原理

1. 插件拦截匹配 `path` 的请求（例如 `/game/assets/sprites/hero.png`）。
2. 从 URL 中剥离挂载路径，得到相对路径（`sprites/hero.png`）。
3. 使用高效的 ZIP 索引在 `zip_file` 中查找对应的文件条目。
4. 如果找到，直接解压该条目数据流并返回给客户端。
5. 如果未找到，返回 404。

## 应用场景

- **热更新资源包**：直接下载并挂载新的 ZIP 资源包，无需繁琐的解压过程。
- **H5 离线包**：将整个 H5 应用打包成 ZIP，直接挂载运行。
- **高性能静态站**：减少大量小文件对文件系统的压力（inode 占用、碎片化）。
