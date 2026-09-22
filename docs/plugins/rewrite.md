# URL Rewrite Plugin

URL Rewrite 插件允许你基于正则表达式修改传入请求的 URL 路径。这对于重定向旧 API 端点、规范化 URL 或将复杂的公共 URL 映射到内部文件结构非常有用。

## 功能特性

- **正则表达式匹配**：使用强大的 Rust 正则表达式引擎进行路径匹配。
- **捕获组支持**：在替换字符串中使用 `$1`, `$2` 等引用正则表达式中的捕获组。
- **多规则支持**：按顺序定义多条重写规则，一旦匹配成功即应用并在当前插件中停止后续匹配（但在整个服务器处理链中继续传递）。
- **无缝集成**：作为请求处理链的第一步运行，重写后的路径将对后续的静态文件服务、WebDAV 或应用程序处理逻辑生效。

## 配置

在服务器配置的 `mounts` 数组中添加类型为 `rewrite` 的项。

```typescript
// 类型定义
interface RewriteRule {
    pattern: string;      // 正则表达式匹配模式
    replacement: string;  // 替换目标字符串
}

interface RewriteMount {
    type: 'rewrite';
    rules: RewriteRule[];
}
```

### 完整配置示例

```json
{
  "root_dir": "./static_files",
  "mounts": [
    {
      "type": "rewrite",
      "rules": [
        {
          "pattern": "^/old-api/(.*)",
          "replacement": "/new-api/$1"
        },
        {
          "pattern": "^/images/(.*)\\.jsp$",
          "replacement": "/static/images/$1.png"
        },
        {
          "pattern": "^/app/(user|admin)/(.*)",
          "replacement": "/internal/$1/handler?path=$2"
        }
      ]
    },
    {
      "type": "static",
      "path": "/static",
      "root": "./assets"
    }
  ]
}
```

## 规则详解

### 1. 简单前缀替换
将所有以 `/old/` 开头的请求重定向到 `/new/`。

- **Pattern**: `^/old/(.*)`
- **Replacement**: `/new/$1`
- **示例**: `/old/users/1` -> `/new/users/1`

### 2. 隐藏文件扩展名
将没有扩展名的请求映射到 `.html` 文件。

- **Pattern**: `^/page/([^/.]+)$`
- **Replacement**: `/pages/$1.html`
- **示例**: `/page/about` -> `/pages/about.html`

### 3. API 版本控制
将 `/v1` API 请求映射到内部具体的服务路径。

- **Pattern**: `^/api/v1/(.*)`
- **Replacement**: `/services/v1_handler/$1`

## 注意事项

1. **执行顺序**：`RewritePlugin` 应该在 `mounts` 列表的早期配置，以确保路径在被其他插件（如 `StaticFilePlugin` 或 `WebDavPlugin`）处理之前被重写。
2. **Path Only**：重写规则仅针对 URL 的路径部分（Path），不包括查询参数（Query String）。查询参数会被自动保留并附加到重写后的路径末尾。
3. **PassThrough**：重写操作是“透传”的。这意味着 URL 被修改后，请求会继续传递给下一个匹配的插件或应用程序的主处理回调。Rewrite 插件本身不会终止请求并返回响应（除非发生错误）。
4. **性能**：规则是按顺序评估的。将最常用的规则放在前面可以微调性能，尽管 Rust 的正则引擎非常高效。

