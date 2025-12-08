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

  std::cout << "[HTTP Server] Sending response for request: " << requestId
            << ", status: " << statusCode << ", headers: " << headersJson
            << ", body length: " << bodyLen << std::endl;

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

    // 解析 headers JSON（简化实现）
    request.headers = std::unordered_map<std::string, std::string>();

    // 设置 body
    if (cRequest->body && cRequest->body_len > 0) {
      request.body = std::string(cRequest->body, cRequest->body_len);
    }

    std::cout << "[HTTP Server] Received request: " << request.method << " "
              << request.path << ", ID: " << request.requestId << std::endl;

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

  return Promise<bool>::async([requestId, statusCode, headersJson,
                               bodyStr]() -> bool {
    const char *body = "";
    size_t bodyLen = 0;

    // 优先使用 binaryBody（二进制数据）
    if (!bodyStr.empty()) {
      body = bodyStr.c_str();
      bodyLen = bodyStr.length();
    }

    std::cout << "[HTTP Server] Sending response (sendResponse) for request: "
              << requestId << ", status: " << statusCode
              << ", headers: " << headersJson << ", body length: " << bodyLen
              << std::endl;

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
    std::cout << "[HTTP Server] sendBinaryResponse: copied " << size
              << " bytes on JS thread" << std::endl;
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

    std::cout
        << "[HTTP Server] sendBinaryResponse: sending response for request "
        << requestId << ", status: " << code << ", body length: " << bodyLen
        << std::endl;

    return send_response(requestId.c_str(), code, headersJson.c_str(), bodyPtr,
                         static_cast<int>(bodyLen));
  });
}

} // namespace margelo::nitro::http_server
