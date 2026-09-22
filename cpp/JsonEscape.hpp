// JSON 字符串编解码（跨 C++ ↔ Rust 的 header JSON 用）
//
//   escapeJsonString   C++ → Rust：把响应头序列化成 JSON 时转义
//   unescapeJsonString Rust → C++：解析请求头 JSON 时还原
//
// 两者必须**对称**：一边转义出来的东西，另一边必须能原样还原。
// 自检里有一条 round-trip 不变量专门钉这个（见 tests/cpp/JsonEscape.test.cpp）。
//
// 之所以单独成一个 header：这两段逻辑原本是 `HybridHttpServer.cpp` 里的 lambda，
// 无法单独编译测试。而它们的错误是**静默**的（整段 JSON 解析失败 → 响应头全丢；
// 或转义序列没还原 → 值悄悄变成字面字符），必须有自动化验证。
// 抽出来自检程序可以直接 include 真实实现：
//   clang++ -std=c++17 -I cpp tests/cpp/JsonEscape.test.cpp -o /tmp/t && /tmp/t
#pragma once

#include <cstdint>
#include <cstdio>
#include <string>

namespace rn_http_server_json {

/// 把字符串转义成合法的 JSON 字符串内容（**不含**两侧引号）。
///
/// 必须转义的：
/// - `"` 与 `\`：JSON 语法本身要求
/// - `\n` `\r` `\t` 及其余 < 0x20 的控制字符：RFC 8259 §7 禁止 JSON 字符串里
///   出现裸控制字符，serde_json 会直接拒收
///
/// 漏掉控制字符的后果**不是**「值不对」，而是**整段 JSON 解析失败**：
/// Rust 侧 `send_response` 里 `serde_json::from_str` 报错后回退成空 HashMap，
/// 于是这一次响应的**所有**响应头被静默丢弃，调用方毫无察觉
/// （只看到响应没有头，看不到任何错误）。
///
/// 不做的事：
/// - 不转义 `/`（JSON 不要求，转义只会引入无谓的 diff）
/// - 不处理 ≥ 0x80 的字节（UTF-8 原样透传；JSON 字符串允许任意 Unicode 码点，
///   只有 < 0x20 是禁区）
inline std::string escapeJsonString(const std::string &str) {
  std::string escaped;
  escaped.reserve(str.size() + 8);

  char buf[8];
  for (char c : str) {
    switch (c) {
    case '"':
      escaped += "\\\"";
      break;
    case '\\':
      escaped += "\\\\";
      break;
    case '\n':
      escaped += "\\n";
      break;
    case '\r':
      escaped += "\\r";
      break;
    case '\t':
      escaped += "\\t";
      break;
    default:
      if (static_cast<unsigned char>(c) < 0x20) {
        // 必须转 unsigned char 再格式化：char 在 arm64/x86 macOS 上是 signed，
        // 0x80-0xFF 会变成负数，%04x 打出来的就是 ffffff80 这种垃圾
        std::snprintf(buf, sizeof(buf), "\\u%04x",
                      static_cast<unsigned char>(c));
        escaped += buf;
      } else {
        escaped += c;
      }
    }
  }
  return escaped;
}

namespace detail {

/// 把一个 Unicode 码点按 UTF-8 追加到 out。
inline void appendUtf8(std::string &out, std::uint32_t cp) {
  if (cp <= 0x7F) {
    out += static_cast<char>(cp);
  } else if (cp <= 0x7FF) {
    out += static_cast<char>(0xC0 | (cp >> 6));
    out += static_cast<char>(0x80 | (cp & 0x3F));
  } else if (cp <= 0xFFFF) {
    out += static_cast<char>(0xE0 | (cp >> 12));
    out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F));
    out += static_cast<char>(0x80 | (cp & 0x3F));
  } else {
    out += static_cast<char>(0xF0 | (cp >> 18));
    out += static_cast<char>(0x80 | ((cp >> 12) & 0x3F));
    out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F));
    out += static_cast<char>(0x80 | (cp & 0x3F));
  }
}

/// 单个十六进制字符 → 数值；非法返回 -1
inline int hexValue(char c) {
  if (c >= '0' && c <= '9')
    return c - '0';
  if (c >= 'a' && c <= 'f')
    return c - 'a' + 10;
  if (c >= 'A' && c <= 'F')
    return c - 'A' + 10;
  return -1;
}

/// 从 str[pos] 开始读 4 个十六进制位（不含 `\u` 前缀）。成功返回 true。
inline bool readHex4(const std::string &str, std::size_t pos,
                     std::uint32_t &out) {
  std::uint32_t v = 0;
  for (std::size_t k = 0; k < 4; k++) {
    if (pos + k >= str.size())
      return false;
    const int h = hexValue(str[pos + k]);
    if (h < 0)
      return false;
    v = (v << 4) | static_cast<std::uint32_t>(h);
  }
  out = v;
  return true;
}

} // namespace detail

/// 还原 JSON 字符串里的转义序列（`\n` `\t` `\r` `\b` `\f` `\"` `\\` `\/` `\uXXXX`）。
///
/// 为什么需要它：Rust 侧用 `serde_json::to_string` 序列化请求头，
/// 它会把 TAB 转义成 `\t`（实测 TAB 是唯一能穿过 `HeaderValue::to_str()`
/// 的控制字符）。不还原的话，header 值会悄悄变成两个字面字符 `\` `t`。
///
/// `\uXXXX` 支持代理对（`\uD83D\uDE00` → U+1F600）；落单的代理项 UTF-8 无法表示，
/// 替换成 U+FFFD 而不是产出非法 UTF-8。
///
/// 未知转义（JSON 不允许）与结尾孤立的 `\` **原样保留**，不吞字符 ——
/// 宁可让上游看到可疑的字面内容，也不要静默丢字节。
inline std::string unescapeJsonString(const std::string &str) {
  std::string out;
  out.reserve(str.size());

  const std::size_t n = str.size();
  for (std::size_t i = 0; i < n; i++) {
    if (str[i] != '\\' || i + 1 >= n) {
      // 普通字符，或结尾孤立的 '\'
      out += str[i];
      continue;
    }

    const char next = str[i + 1];
    switch (next) {
    case '"':
      out += '"';
      i++;
      break;
    case '\\':
      out += '\\';
      i++;
      break;
    case '/':
      out += '/';
      i++;
      break;
    case 'b':
      out += '\b';
      i++;
      break;
    case 'f':
      out += '\f';
      i++;
      break;
    case 'n':
      out += '\n';
      i++;
      break;
    case 'r':
      out += '\r';
      i++;
      break;
    case 't':
      out += '\t';
      i++;
      break;
    case 'u': {
      std::uint32_t cp = 0;
      // 需要 str[i+2 .. i+5] 四个十六进制位
      if (!detail::readHex4(str, i + 2, cp)) {
        out += '\\';
        out += next;
        i++;
        break;
      }
      i += 5; // i 落在最后一个十六进制位上，循环的 i++ 会跳过它

      // 代理对：高位代理后面必须紧跟 \uDC00-\uDFFF。
      // 注意此时的 i 停在**第一个转义的最后一个十六进制位**上，所以下一个转义的
      // `\` 在 i+1、`u` 在 i+2、四个十六进制位在 i+3..i+6。
      if (cp >= 0xD800 && cp <= 0xDBFF && i + 2 < n && str[i + 1] == '\\' &&
          str[i + 2] == 'u' && i + 6 < n) {
        std::uint32_t lo = 0;
        if (detail::readHex4(str, i + 3, lo) && lo >= 0xDC00 && lo <= 0xDFFF) {
          cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
          i += 6;
        }
      }
      if (cp >= 0xD800 && cp <= 0xDFFF) {
        cp = 0xFFFD; // 落单代理项：UTF-8 表示不了，替换
      }
      detail::appendUtf8(out, cp);
      break;
    }
    default:
      // JSON 不允许未知转义。原样保留（含反斜杠），不吞字符
      out += '\\';
      out += next;
      i++;
      break;
    }
  }
  return out;
}

} // namespace rn_http_server_json
