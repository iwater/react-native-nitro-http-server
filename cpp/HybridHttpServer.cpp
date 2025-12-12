// cpp/HybridHttpServer.cpp
#include "HybridHttpServer.hpp"
#include <iostream>
#include <mutex>
#include <unordered_map>
#include <vector>

extern "C" {
#include "rn_http_server.h"
}

namespace margelo::nitro::http_server {

using namespace margelo::nitro;

// 定义回调函数类型别名
using HandlerType = std::function<std::shared_ptr<Promise<
    std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(
    const HttpRequest &)>;

// 全局管理器：存储回调函数和服务器实例
struct ServerContext {
  HandlerType handler;
  std::mutex mutex;
};

static ServerContext *g_serverContext = nullptr;
static std::mutex g_contextMutex;

// 辅助函数：序列化 headers 为 JSON 字符串
static std::string serializeHeaders(
    const std::optional<std::unordered_map<std::string, std::string>>
        &headers) {
  if (!headers.has_value() || headers.value().empty()) {
    return "{}";
  }

  std::string json = "{";
  bool first = true;
  for (const auto &[key, value] : headers.value()) {
    if (!first)
      json += ",";
    first = false;

    // 简单的 JSON 字符串转义（处理引号和反斜杠）
    auto escapeJson = [](const std::string &str) -> std::string {
      std::string escaped;
      for (char c : str) {
        if (c == '"' || c == '\\') {
          escaped += '\\';
        }
        escaped += c;
      }
      return escaped;
    };

    json += "\"" + escapeJson(key) + "\":\"" + escapeJson(value) + "\"";
  }
  json += "}";
  return json;
}

// 辅助函数：从 HttpResponse 提取数据并发送 HTTP 响应
// 警告：此函数在回调路径中调用，可能在非 JS 线程上执行
// 因此 **不能** 访问 ArrayBuffer（binaryBody），否则会导致内存损坏
static void extractAndSendResponse(const std::string &requestId,
                                   const HttpResponse &response) {
  int statusCode = static_cast<int>(response.statusCode);

  // 序列化 headers
  std::string headersJson = serializeHeaders(response.headers);

  // 仅使用字符串 body
  std::string bodyStr;
  if (response.body.has_value()) {
    bodyStr = response.body.value();
  }

  const char *body = "";
  size_t bodyLen = 0;

  if (!bodyStr.empty()) {
    body = bodyStr.c_str();
    bodyLen = bodyStr.length();
  }

  // std::cout << "[HTTP Server] Sending response for request: " << requestId
  //           << ", status: " << statusCode << ", headers: " << headersJson
  //           << ", body length: " << bodyLen << std::endl;

  // 直接发送响应（send_response 内部会将数据复制到 Rust）
  send_response(requestId.c_str(), statusCode, headersJson.c_str(), body,
                static_cast<int>(bodyLen));
}

// C 回调函数：从 Rust 服务器调用
static void c_request_callback(::HttpRequest *cRequest) {
  if (!cRequest) {
    return;
  }

  HandlerType handler;
  {
    std::lock_guard<std::mutex> lock(g_contextMutex);
    if (!g_serverContext || !g_serverContext->handler) {
      free_http_request(cRequest);
      return;
    }
    handler = g_serverContext->handler;
  }

  try {
    // 转换 C 结构体到 C++ 结构体
    HttpRequest request;

    if (cRequest->request_id) {
      request.requestId = std::string(cRequest->request_id);
    }

    if (cRequest->method) {
      request.method = std::string(cRequest->method);
    }

    if (cRequest->path) {
      request.path = std::string(cRequest->path);
    }

    // Parse headers JSON
    request.headers = std::unordered_map<std::string, std::string>();
    if (cRequest->headers_json && strlen(cRequest->headers_json) > 0) {
      std::string jsonStr(cRequest->headers_json);

      // Simple JSON parser for headers (assumes well-formed JSON object)
      // Format: {"key1":"value1","key2":"value2"}
      if (jsonStr.length() >= 2 && jsonStr[0] == '{' &&
          jsonStr[jsonStr.length() - 1] == '}') {
        size_t pos = 1; // Skip opening '{'

        while (pos < jsonStr.length() - 1) {
          // Skip whitespace
          while (pos < jsonStr.length() && std::isspace(jsonStr[pos]))
            pos++;

          if (pos >= jsonStr.length() - 1 || jsonStr[pos] == '}')
            break;

          // Parse key
          if (jsonStr[pos] != '"')
            break; // Expect quoted key
          pos++;   // Skip opening quote

          size_t keyStart = pos;
          while (pos < jsonStr.length() && jsonStr[pos] != '"') {
            if (jsonStr[pos] == '\\' && pos + 1 < jsonStr.length()) {
              pos += 2; // Skip escaped character
            } else {
              pos++;
            }
          }

          if (pos >= jsonStr.length())
            break;
          std::string key = jsonStr.substr(keyStart, pos - keyStart);
          pos++; // Skip closing quote

          // Skip whitespace and colon
          while (pos < jsonStr.length() &&
                 (std::isspace(jsonStr[pos]) || jsonStr[pos] == ':'))
            pos++;

          // Parse value
          if (pos >= jsonStr.length() || jsonStr[pos] != '"')
            break; // Expect quoted value
          pos++;   // Skip opening quote

          size_t valueStart = pos;
          while (pos < jsonStr.length() && jsonStr[pos] != '"') {
            if (jsonStr[pos] == '\\' && pos + 1 < jsonStr.length()) {
              pos += 2; // Skip escaped character
            } else {
              pos++;
            }
          }

          if (pos >= jsonStr.length())
            break;
          std::string value = jsonStr.substr(valueStart, pos - valueStart);
          pos++; // Skip closing quote

          // Unescape common JSON escape sequences
          auto unescape = [](const std::string &str) -> std::string {
            std::string result;
            for (size_t i = 0; i < str.length(); i++) {
              if (str[i] == '\\' && i + 1 < str.length()) {
                char next = str[i + 1];
                if (next == '"' || next == '\\' || next == '/') {
                  result += next;
                  i++;
                } else if (next == 'n') {
                  result += '\n';
                  i++;
                } else if (next == 't') {
                  result += '\t';
                  i++;
                } else {
                  result += str[i];
                }
              } else {
                result += str[i];
              }
            }
            return result;
          };

          request.headers[unescape(key)] = unescape(value);

          // Skip whitespace and comma
          while (pos < jsonStr.length() &&
                 (std::isspace(jsonStr[pos]) || jsonStr[pos] == ','))
            pos++;
        }
      }
    }

    // Set body - check if this is a buffer upload request
    // Buffer upload requests have X-Upload-Filename header set by the plugin
    bool isBufferUpload = false;
    if (request.headers.find("x-upload-filename") != request.headers.end()) {
      isBufferUpload = true;
    }

    if (cRequest->body && cRequest->body_len > 0) {
      if (isBufferUpload) {
        // For buffer upload, create an ArrayBuffer to hold the binary data
        // Use ArrayBuffer::copy to safely copy the data
        size_t size = static_cast<size_t>(cRequest->body_len);
        auto buffer = ArrayBuffer::copy(
            reinterpret_cast<const uint8_t *>(cRequest->body), size);
        request.binaryBody = buffer;

        // std::cout
        //     << "[HTTP Server] Buffer upload detected, created ArrayBuffer
        //     with "
        //     << size << " bytes" << std::endl;
      } else {
        // Regular string body
        request.body = std::string(cRequest->body, cRequest->body_len);
      }
    }

    // std::cout << "[HTTP Server] Received request: " << request.method << " "
    //           << request.path << ", ID: " << request.requestId << std::endl;

    // 保存 requestId 用于后续响应
    std::string requestId = request.requestId;

    // 调用 JavaScript 回调
    auto responsePromise = handler(request);

    // 使用 addOnResolvedListener 处理 Promise 结果
    responsePromise->addOnResolvedListener(
        [requestId](
            const std::variant<
                HttpResponse, std::shared_ptr<Promise<HttpResponse>>> &result) {
          // 处理返回值（可能是直接的响应或者 Promise）
          if (std::holds_alternative<HttpResponse>(result)) {
            HttpResponse response = std::get<HttpResponse>(result);
            extractAndSendResponse(requestId, response);
          } else {
            // 如果是 Promise，等待它完成
            auto promise =
                std::get<std::shared_ptr<Promise<HttpResponse>>>(result);
            promise->addOnResolvedListener(
                [requestId](const HttpResponse &resp) {
                  extractAndSendResponse(requestId, resp);
                });
            promise->addOnRejectedListener(
                [requestId](const std::exception_ptr &error) {
                  // 发送错误响应
                  HttpResponse errorResp;
                  errorResp.statusCode = 500;
                  errorResp.body = "Internal Server Error";
                  extractAndSendResponse(requestId, errorResp);
                });
          }
        });

    responsePromise->addOnRejectedListener(
        [requestId](const std::exception_ptr &error) {
          // 发送错误响应
          HttpResponse errorResp;
          errorResp.statusCode = 500;
          errorResp.body = "Internal Server Error";
          extractAndSendResponse(requestId, errorResp);
        });

  } catch (const std::exception &e) {
    std::cerr << "Error in c_request_callback: " << e.what() << std::endl;
  }

  // 释放 C 请求资源
  free_http_request(cRequest);
}

HybridHttpServer::HybridHttpServer() : HybridObject(TAG) {
  // Constructor
}

HybridHttpServer::~HybridHttpServer() {
  // 清理
  std::lock_guard<std::mutex> lock(g_contextMutex);
  if (g_serverContext) {
    delete g_serverContext;
    g_serverContext = nullptr;
  }
}

std::shared_ptr<Promise<bool>> HybridHttpServer::start(
    double port,
    const std::function<std::shared_ptr<Promise<
        std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(
        const HttpRequest &)> &handler,
    const std::optional<std::string> &host) {
  return Promise<bool>::async([port, handler, host]() -> bool {
    // 创建或更新全局上下文
    {
      std::lock_guard<std::mutex> lock(g_contextMutex);
      if (!g_serverContext) {
        g_serverContext = new ServerContext();
      }
      g_serverContext->handler = handler;
    }

    // 启动服务器
    int portInt = static_cast<int>(port);
    const char *hostCStr = host.has_value() ? host.value().c_str() : nullptr;
    bool success = start_server(portInt, hostCStr, c_request_callback);

    return success;
  });
}

std::shared_ptr<Promise<bool>>
HybridHttpServer::sendResponse(const std::string &requestId,
                               const HttpResponse &response) {

  // 复制其他需要的数据
  std::string bodyStr = response.body.has_value() ? response.body.value() : "";
  std::string headersJson = serializeHeaders(response.headers);
  int statusCode = static_cast<int>(response.statusCode);

  return Promise<bool>::async(
      [requestId, statusCode, headersJson, bodyStr]() -> bool {
        const char *body = "";
        size_t bodyLen = 0;

        // 优先使用 binaryBody（二进制数据）
        if (!bodyStr.empty()) {
          body = bodyStr.c_str();
          bodyLen = bodyStr.length();
        }

        // std::cout << "[HTTP Server] Sending response (sendResponse) for
        // request: "
        //           << requestId << ", status: " << statusCode
        //           << ", headers: " << headersJson << ", body length: " <<
        //           bodyLen
        //           << std::endl;

        return send_response(requestId.c_str(), statusCode, headersJson.c_str(),
                             body, static_cast<int>(bodyLen));
      });
}

std::shared_ptr<Promise<void>> HybridHttpServer::stop() {
  return Promise<void>::async([]() {
    stop_server();

    // 清理回调
    std::lock_guard<std::mutex> lock(g_contextMutex);
    if (g_serverContext) {
      g_serverContext->handler = nullptr;
    }
  });
}

std::shared_ptr<Promise<ServerStats>> HybridHttpServer::getStats() {
  return Promise<ServerStats>::async([]() -> ServerStats {
    const char *statsJson = get_server_stats();

    // TODO: 解析 JSON 字符串为 ServerStats 结构体
    // 暂时返回默认值
    ServerStats stats;
    stats.totalRequests = 0;
    stats.activeConnections = 0;
    stats.bytesSent = 0;
    stats.bytesReceived = 0;
    stats.uptime = 0;
    stats.errorCount = 0;

    return stats;
  });
}

std::shared_ptr<Promise<bool>> HybridHttpServer::isRunning() {
  return Promise<bool>::async([]() -> bool {
    // 简单实现：检查全局上下文是否存在且有回调
    std::lock_guard<std::mutex> lock(g_contextMutex);
    return g_serverContext != nullptr && g_serverContext->handler != nullptr;
  });
}

std::shared_ptr<Promise<bool>>
HybridHttpServer::startStaticServer(double port, const std::string &rootDir,
                                    const std::optional<std::string> &host) {
  return Promise<bool>::async([port, rootDir, host]() -> bool {
    int portInt = static_cast<int>(port);
    const char *hostCStr = host.has_value() ? host.value().c_str() : nullptr;
    return start_static_server(portInt, hostCStr, rootDir.c_str());
  });
}

std::shared_ptr<Promise<void>> HybridHttpServer::stopStaticServer() {
  return Promise<void>::async([]() { stop_static_server(); });
}

std::shared_ptr<Promise<bool>> HybridHttpServer::startAppServer(
    double port, const std::string &rootDir,
    const std::function<std::shared_ptr<Promise<
        std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(
        const HttpRequest &)> &handler,
    const std::optional<std::string> &host) {
  return Promise<bool>::async([port, rootDir, handler, host]() -> bool {
    // Create or update global context
    {
      std::lock_guard<std::mutex> lock(g_contextMutex);
      if (!g_serverContext) {
        g_serverContext = new ServerContext();
      }
      g_serverContext->handler = handler;
    }

    // Start server
    int portInt = static_cast<int>(port);
    const char *hostCStr = host.has_value() ? host.value().c_str() : nullptr;
    // Using "start_app_server" from C library
    bool success = start_app_server(portInt, hostCStr, rootDir.c_str(),
                                    c_request_callback);

    return success;
  });
}

std::shared_ptr<Promise<void>> HybridHttpServer::stopAppServer() {
  return Promise<void>::async([]() {
    stop_app_server();

    // Clean up callback
    std::lock_guard<std::mutex> lock(g_contextMutex);
    if (g_serverContext) {
      g_serverContext->handler = nullptr;
    }
  });
}

std::shared_ptr<Promise<bool>> HybridHttpServer::startServerWithConfig(
    double port,
    const std::function<std::shared_ptr<Promise<
        std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(
        const HttpRequest &)> &handler,
    const std::string &configJson, const std::optional<std::string> &host) {
  return Promise<bool>::async([port, handler, configJson, host]() -> bool {
    // Create or update global context
    {
      std::lock_guard<std::mutex> lock(g_contextMutex);
      if (!g_serverContext) {
        g_serverContext = new ServerContext();
      }
      g_serverContext->handler = handler;
    }

    // Start server with config
    int portInt = static_cast<int>(port);
    const char *hostCStr = host.has_value() ? host.value().c_str() : nullptr;
    bool success = start_server_with_config(
        portInt, hostCStr, c_request_callback, configJson.c_str());

    return success;
  });
}

void HybridHttpServer::sendHttpResponse(const std::string &requestId,
                                        const HttpResponse &response) {
  extractAndSendResponse(requestId, response);
}

std::shared_ptr<Promise<std::string>>
HybridHttpServer::readRequestBodyChunk(const std::string &requestId) {
  return Promise<std::string>::async([requestId]() -> std::string {
    // Buffer size 64KB
    const int BUFFER_SIZE = 64 * 1024;
    std::vector<char> buffer(BUFFER_SIZE);

    int bytesRead =
        read_request_body_chunk(requestId.c_str(), buffer.data(), BUFFER_SIZE);

    if (bytesRead < 0) {
      throw std::runtime_error("Failed to read request body chunk");
    } else if (bytesRead == 0) {
      return "";
    } else {
      return std::string(buffer.data(), bytesRead);
    }
  });
}

std::shared_ptr<Promise<bool>>
HybridHttpServer::writeResponseChunk(const std::string &requestId,
                                     const std::string &chunk) {
  return Promise<bool>::async([requestId, chunk]() -> bool {
    return write_response_chunk(requestId.c_str(), chunk.c_str(),
                                static_cast<int>(chunk.length()));
  });
}

std::shared_ptr<Promise<bool>>
HybridHttpServer::endResponse(const std::string &requestId, double statusCode,
                              const std::string &headersJson) {
  return Promise<bool>::async([requestId, statusCode, headersJson]() -> bool {
    int code = static_cast<int>(statusCode);
    return end_response(requestId.c_str(), code, headersJson.c_str());
  });
}

std::shared_ptr<Promise<bool>> HybridHttpServer::sendBinaryResponse(
    const std::string &requestId, double statusCode,
    const std::string &headersJson, const std::shared_ptr<ArrayBuffer> &body) {
  // 关键：在 JS 线程上同步复制 ArrayBuffer 数据
  // 这样可以确保在进入异步上下文之前数据已被安全复制
  std::vector<uint8_t> binaryData;
  if (body && body->data() && body->size() > 0) {
    const uint8_t *data = body->data();
    size_t size = body->size();
    binaryData.assign(data, data + size);
    // std::cout << "[HTTP Server] sendBinaryResponse: copied " << size
    //           << " bytes on JS thread" << std::endl;
  }

  int code = static_cast<int>(statusCode);

  // 使用移动语义将数据传入异步上下文
  return Promise<bool>::async([requestId, code, headersJson,
                               binaryData = std::move(binaryData)]() -> bool {
    const char *bodyPtr = "";
    size_t bodyLen = 0;

    if (!binaryData.empty()) {
      bodyPtr = reinterpret_cast<const char *>(binaryData.data());
      bodyLen = binaryData.size();
    }

    // std::cout
    //     << "[HTTP Server] sendBinaryResponse: sending response for request "
    //     << requestId << ", status: " << code << ", body length: " << bodyLen
    //     << std::endl;

    return send_response(requestId.c_str(), code, headersJson.c_str(), bodyPtr,
                         static_cast<int>(bodyLen));
  });
}

// ==================== WebSocket API ====================

// 全局 WebSocket 事件处理器
static std::function<void(const WebSocketEvent &)> g_wsHandler;
static std::mutex g_wsHandlerMutex;

// C 回调函数：从 Rust 服务器调用
static void c_websocket_callback(const ::WebSocketEvent *cEvent) {
  if (!cEvent) {
    return;
  }

  std::function<void(const WebSocketEvent &)> handler;
  {
    std::lock_guard<std::mutex> lock(g_wsHandlerMutex);
    if (!g_wsHandler) {
      return;
    }
    handler = g_wsHandler;
  }

  try {
    WebSocketEvent event;

    if (cEvent->connection_id) {
      event.connectionId = std::string(cEvent->connection_id);
    }

    // 转换事件类型
    switch (cEvent->event_type) {
    case 1:
      event.type = WebSocketEventType::OPEN;
      break;
    case 2:
      event.type = WebSocketEventType::MESSAGE;
      break;
    case 3:
      event.type = WebSocketEventType::CLOSE;
      break;
    case 4:
      event.type = WebSocketEventType::ERROR;
      break;
    default:
      return;
    }

    // 路径
    if (cEvent->path) {
      event.path = std::string(cEvent->path);
    }

    // 查询字符串
    if (cEvent->query) {
      event.query = std::string(cEvent->query);
    }

    // HTTP 头 JSON
    if (cEvent->headers_json) {
      event.headersJson = std::string(cEvent->headers_json);
    }

    // 文本数据
    if (cEvent->text_data && cEvent->text_len > 0) {
      event.textData = std::string(cEvent->text_data, cEvent->text_len);
    }

    // 二进制数据
    if (cEvent->binary_data && cEvent->binary_len > 0) {
      size_t size = static_cast<size_t>(cEvent->binary_len);
      auto buffer = ArrayBuffer::copy(
          reinterpret_cast<const uint8_t *>(cEvent->binary_data), size);
      event.binaryData = buffer;
    }

    // 关闭代码和原因
    if (cEvent->close_code > 0) {
      event.closeCode = static_cast<double>(cEvent->close_code);
    }
    if (cEvent->close_reason) {
      event.closeReason = std::string(cEvent->close_reason);
    }

    // 调用 JavaScript 处理器
    handler(event);

  } catch (const std::exception &e) {
    std::cerr << "Error in c_websocket_callback: " << e.what() << std::endl;
  }
}

void HybridHttpServer::setWebSocketHandler(
    const std::function<void(const WebSocketEvent &)> &handler) {
  // 保存处理器
  {
    std::lock_guard<std::mutex> lock(g_wsHandlerMutex);
    g_wsHandler = handler;
  }
  _wsHandler = handler;

  // 设置 C 回调
  set_websocket_callback(c_websocket_callback);
}

std::shared_ptr<Promise<bool>>
HybridHttpServer::wsSendText(const std::string &connectionId,
                             const std::string &message) {
  return Promise<bool>::async([connectionId, message]() -> bool {
    return ws_send_text(connectionId.c_str(), message.c_str());
  });
}

std::shared_ptr<Promise<bool>>
HybridHttpServer::wsSendBinary(const std::string &connectionId,
                               const std::shared_ptr<ArrayBuffer> &data) {
  // 在 JS 线程上同步复制数据
  std::vector<uint8_t> binaryData;
  if (data && data->data() && data->size() > 0) {
    const uint8_t *dataPtr = data->data();
    size_t size = data->size();
    binaryData.assign(dataPtr, dataPtr + size);
  }

  return Promise<bool>::async(
      [connectionId, binaryData = std::move(binaryData)]() -> bool {
        if (binaryData.empty()) {
          return false;
        }
        return ws_send_binary(connectionId.c_str(),
                              reinterpret_cast<const char *>(binaryData.data()),
                              static_cast<int>(binaryData.size()));
      });
}

std::shared_ptr<Promise<bool>>
HybridHttpServer::wsClose(const std::string &connectionId,
                          std::optional<double> code,
                          const std::optional<std::string> &reason) {
  int closeCode = code.has_value() ? static_cast<int>(code.value()) : 1000;
  std::string closeReason = reason.has_value() ? reason.value() : "";

  return Promise<bool>::async([connectionId, closeCode, closeReason]() -> bool {
    return ws_close(connectionId.c_str(), closeCode, closeReason.c_str());
  });
}

} // namespace margelo::nitro::http_server
