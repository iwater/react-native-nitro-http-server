// cpp/HybridHttpServer.hpp
#pragma once
#include "../nitrogen/generated/shared/c++/HybridHttpServerSpec.hpp"
#include <NitroModules/ArrayBuffer.hpp>

namespace margelo::nitro::http_server {

class HybridHttpServer : public HybridHttpServerSpec {
public:
  HybridHttpServer();
  ~HybridHttpServer() override;

  // 实现 HybridHttpServerSpec 接口
  std::shared_ptr<Promise<bool>> start(
      double port,
      const std::function<std::shared_ptr<Promise<
          std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(
          const HttpRequest &)> &handler,
      const std::optional<std::string> &host) override;

  std::shared_ptr<Promise<bool>>
  sendResponse(const std::string &requestId,
               const HttpResponse &response) override;

  std::shared_ptr<Promise<void>> stop() override;

  std::shared_ptr<Promise<ServerStats>> getStats() override;

  std::shared_ptr<Promise<bool>> isRunning() override;

  // 静态服务器方法
  std::shared_ptr<Promise<bool>>
  startStaticServer(double port, const std::string &rootDir,
                    const std::optional<std::string> &host) override;

  std::shared_ptr<Promise<void>> stopStaticServer() override;

  // App server methods
  std::shared_ptr<Promise<bool>> startAppServer(
      double port, const std::string &rootDir,
      const std::function<std::shared_ptr<Promise<
          std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(
          const HttpRequest &)> &handler,
      const std::optional<std::string> &host) override;

  std::shared_ptr<Promise<void>> stopAppServer() override;

  // 带配置的 App server 方法
  std::shared_ptr<Promise<bool>> startServerWithConfig(
      double port, const std::string &rootDir,
      const std::function<std::shared_ptr<Promise<
          std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(
          const HttpRequest &)> &handler,
      const std::string &configJson,
      const std::optional<std::string> &host) override;

  // 流式接口
  std::shared_ptr<Promise<std::string>>
  readRequestBodyChunk(const std::string &requestId) override;
  std::shared_ptr<Promise<bool>>
  writeResponseChunk(const std::string &requestId,
                     const std::string &chunk) override;
  std::shared_ptr<Promise<bool>>
  endResponse(const std::string &requestId, double statusCode,
              const std::string &headersJson) override;

  // 二进制响应（在 JS 线程上安全复制数据）
  std::shared_ptr<Promise<bool>>
  sendBinaryResponse(const std::string &requestId, double statusCode,
                     const std::string &headersJson,
                     const std::shared_ptr<ArrayBuffer> &body) override;

private:
  // 发送 HTTP 响应到 Rust 服务器的辅助方法
  void sendHttpResponse(const std::string &requestId,
                        const HttpResponse &response);
};

} // namespace margelo::nitro::http_server
