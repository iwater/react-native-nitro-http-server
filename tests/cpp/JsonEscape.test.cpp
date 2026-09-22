// JsonEscape 的自检程序（不需要任何测试框架）
//
// 运行方式（在包根目录）：
//   clang++ -std=c++17 -Wall -Wextra -I cpp tests/cpp/JsonEscape.test.cpp -o /tmp/json_escape_test
//   /tmp/json_escape_test
//
// 退出码 0 = 全通过；非 0 = 有失败。
//
// 为什么它放在 tests/cpp 而不是 cpp/：
// podspec 的 source_files 是 "cpp/**/*.{hpp,cpp,h,mm}"，放进去会被编进库里，
// 而这个文件有 main()，会导致重复符号。tests/ 不在任何构建的 glob 范围内。

#include "JsonEscape.hpp"

#include <cstdio>
#include <initializer_list>
#include <string>
#include <vector>

namespace {

int g_failed = 0;
int g_passed = 0;

/// 从显式字节列表建字符串。
///
/// 必须用它，不要写 `"a\x01b"` 这种字面量：C++ 的 `\x` 转义是**贪婪**的，
/// `\x01b` 会把 `b` 也当成十六进制数字 → 解析成 0x1b。也就是说
/// `"a\x01b"` 静默地变成了别的字节，测试测的是错东西（0x1f/0x7f 那两个
/// 则因为超出范围直接编译报错，反而更容易发现）。
std::string bytes(std::initializer_list<unsigned char> bs) {
  return std::string(bs.begin(), bs.end());
}

void check_eq(const std::string &actual, const std::string &expected,
              const char *what) {
  if (actual == expected) {
    g_passed++;
  } else {
    g_failed++;
    std::printf("FAIL %s\n  expected: %s\n  actual:   %s\n", what,
                expected.c_str(), actual.c_str());
  }
}

/// 最关键的不变量：转义结果里**不能**残留任何 < 0x20 的裸控制字符。
/// JSON 规范（RFC 8259 §7）禁止字符串里出现裸控制字符；serde_json 会直接拒收，
/// 于是整段 headers JSON 解析失败 → Rust 侧回退成空 HashMap →
/// 这一次响应的**所有**响应头被静默丢弃。
void check_no_raw_control(const std::string &s, const char *what) {
  for (unsigned char c : s) {
    if (c < 0x20) {
      g_failed++;
      std::printf("FAIL %s: 转义结果里残留裸控制字符 0x%02x\n", what, c);
      return;
    }
  }
  g_passed++;
}

} // namespace

int main() {
  using rn_http_server_json::escapeJsonString;

  // --- 语法必需的转义 ---
  check_eq(escapeJsonString("a\"b"), "a\\\"b", "双引号");
  check_eq(escapeJsonString("a\\b"), "a\\\\b", "反斜杠");

  // --- 控制字符（本次修复的重点）---
  check_eq(escapeJsonString("line1\nline2"), "line1\\nline2", "换行 LF");
  check_eq(escapeJsonString("a\rb"), "a\\rb", "回车 CR");
  check_eq(escapeJsonString("a\tb"), "a\\tb", "制表符 TAB");
  check_eq(escapeJsonString(bytes({'a', 0x01, 'b'})), "a\\u0001b", "0x01");
  check_eq(escapeJsonString(bytes({'a', 0x1f, 'b'})), "a\\u001fb", "0x1f");

  // --- 不该动的内容 ---
  check_eq(escapeJsonString(""), "", "空串");
  check_eq(escapeJsonString("plain text 123"), "plain text 123", "普通 ASCII");
  // DEL(0x7f) ≥ 0x20，JSON 允许裸放
  check_eq(escapeJsonString(bytes({'a', 0x7f, 'b'})), bytes({'a', 0x7f, 'b'}),
           "0x7f DEL 不转义");
  // UTF-8 多字节（字节 ≥ 0x80 原样透传）
  check_eq(escapeJsonString(bytes({0xe6, 0x8a, 0xa5, 0xe5, 0x91, 0x8a})),
           bytes({0xe6, 0x8a, 0xa5, 0xe5, 0x91, 0x8a}), "UTF-8 中文原样透传");
  // '/' JSON 里不要求转义，保持原样（不要引入无谓的 diff）
  check_eq(escapeJsonString("a/b"), "a/b", "斜杠不转义");

  // --- 不变量：结果里不能有裸控制字符 ---
  check_no_raw_control(escapeJsonString("a\nb"), "不变量: LF");
  check_no_raw_control(escapeJsonString("a\r\tb"), "不变量: CR+TAB");
  check_no_raw_control(escapeJsonString(bytes({0x00, 0x01, 0x1f})),
                       "不变量: 0x00-0x1f");
  check_no_raw_control(
      escapeJsonString(bytes({0xe6, 0x8a, 0xa5, '\n', 0xe5, 0x91, 0x8a})),
      "不变量: 中文+LF");

  // ===== unescapeJsonString：与 escapeJsonString 对称的另一半 =====
  using rn_http_server_json::unescapeJsonString;

  // --- 基础转义 ---
  check_eq(unescapeJsonString("a\\\"b"), "a\"b", "还原 \\\"");
  check_eq(unescapeJsonString("a\\\\b"), "a\\b", "还原 \\\\");
  check_eq(unescapeJsonString("a\\/b"), "a/b", "还原 \\/");
  check_eq(unescapeJsonString("a\\nb"), "a\nb", "还原 \\n");
  check_eq(unescapeJsonString("a\\tb"), "a\tb", "还原 \\t");

  // --- 修复前漏掉的（旧实现会原样留成两个字面字符）---
  check_eq(unescapeJsonString("a\\rb"), "a\rb", "还原 \\r（旧实现漏了）");
  check_eq(unescapeJsonString("a\\bb"), std::string("a\bb"), "还原 \\b（旧实现漏了）");
  check_eq(unescapeJsonString("a\\fb"), std::string("a\fb"), "还原 \\f（旧实现漏了）");
  check_eq(unescapeJsonString("a\\u0001b"), bytes({'a', 0x01, 'b'}),
           "还原 \\u0001（旧实现漏了）");
  check_eq(unescapeJsonString("a\\u001Fb"), bytes({'a', 0x1f, 'b'}),
           "还原 \\u001F（十六进制大小写都要认）");
  check_eq(unescapeJsonString("\\u4e2d\\u6587"), bytes({0xe4, 0xb8, 0xad, 0xe6, 0x96, 0x87}),
           "还原 \\uXXXX 非 ASCII（中文）");
  // 代理对 → U+1F600 😀
  check_eq(unescapeJsonString("\\ud83d\\ude00"),
           bytes({0xf0, 0x9f, 0x98, 0x80}), "还原代理对 \\uD83D\\uDE00");
  // 落单的代理项：UTF-8 表示不了 → U+FFFD，不能产出非法 UTF-8
  check_eq(unescapeJsonString("\\ud83d"), bytes({0xef, 0xbf, 0xbd}),
           "落单代理项 → U+FFFD");

  // --- 不该动的内容 ---
  check_eq(unescapeJsonString(""), "", "空串");
  check_eq(unescapeJsonString("plain text"), "plain text", "普通 ASCII 不变");
  check_eq(unescapeJsonString(bytes({0xe6, 0x8a, 0xa5})), bytes({0xe6, 0x8a, 0xa5}),
           "已经是 UTF-8 的字节原样透传");
  // 未知转义与结尾孤立反斜杠：原样保留，不吞字符
  check_eq(unescapeJsonString("a\\xb"), "a\\xb", "未知转义原样保留");
  check_eq(unescapeJsonString("a\\"), "a\\", "结尾孤立反斜杠原样保留");
  check_eq(unescapeJsonString("a\\u12"), "a\\u12", "\\u 后不足 4 位 → 原样保留");

  // --- round-trip 不变量：这是「两侧对称」最直接的证据 ---
  // 任何一个字符串，转义再还原必须一模一样。
  const std::vector<std::string> round_trip_inputs = {
      "",
      "plain",
      "with space",
      "quote\" and backslash\\",
      "line1\nline2",
      "tab\there",
      "cr\rand\bback\fform",
      bytes({'a', 0x01, 0x1f, 'b'}),
      bytes({0xe6, 0x8a, 0xa5, 0xe5, 0x91, 0x8a}), // 报告
      bytes({0xf0, 0x9f, 0x98, 0x80}),             // 😀
      "slash/kept",
  };
  for (const auto &input : round_trip_inputs) {
    const std::string escaped = escapeJsonString(input);
    check_no_raw_control(escaped, "round-trip: 转义结果无裸控制字符");
    check_eq(unescapeJsonString(escaped), input,
             "round-trip: unescape(escape(x)) == x");
  }

  std::printf("\nJsonEscape: %d passed, %d failed\n", g_passed, g_failed);
  return g_failed == 0 ? 0 : 1;
}
