# Static File Plugin

Static File 插件是 HTTP 服务器的核心插件，用于提供静态文件服务（HTML, CSS, JS, 图片等）以及目录列表功能。

## 功能特性

- **高性能文件传输**：基于 Rust 的异步 IO 高效分发文件。
- **自动 MIME 类型**：根据文件扩展名自动推断 Content-Type。
- **默认索引文件**：支持自动查找 `index.html`, `index.htm` 等默认文件。
- **目录列表**：支持生成美观的 HTML 目录文件列表（可选）。
- **路径安全**：内置路径遍历防护，防止访问根目录之外的文件。

## 配置

在服务器配置的 `mounts` 数组中添加类型为 `static` 的项。

```typescript
// 类型定义
interface DirListConfig {
    enabled?: boolean;     // 是否启用目录列表
    show_hidden?: boolean; // 是否显示隐藏文件 (以 . 开头的文件)
}

interface StaticMount {
  type: 'static';
  path: string;           // 挂载的 URL 路径
  root: string;           // 本地文件系统目录路径
  dir_list?: DirListConfig; // 目录列表配置 (可选)
  default_index?: string[]; // 默认索引文件名列表 (可选)
}
```

### 完整配置示例

```json
{
  "mounts": [
    {
      "type": "static",
      "path": "/assets",
      "root": "./public/assets"
    },
    {
      "type": "static",
      "path": "/",
      "root": "./www",
      "dir_list": {
        "enabled": true,
        "show_hidden": false
      },
      "default_index": ["index.html", "main.html"]
    }
  ]
}
```

## 配置项详解

### `path`
URL 挂载点。所有以该路径开头的请求都将由本插件处理。
- 示例: `/static` -> `http://localhost/static/style.css`

### `root`
本地文件系统的根目录路径。可以是绝对路径或相对于服务器工作目录的相对路径。
- 示例: `./www`

### `dir_list`
控制当请求指向目录且没有找到默认索引文件时，是否显示文件列表。
- `enabled`: `true` 启用 HTML 目录列表渲染，`false` 返回 403 (默认)。
- `show_hidden`: 是否显示以点 (`.`) 开头的文件。

### `default_index`
自定义查找的默认文件名列表。
- 默认值: `["index.html", "index.htm"]`。
- 如果请求目录（如 `/docs/`），插件会按顺序查找这些文件，如果找到则直接返回内容，而不是显示目录列表。
