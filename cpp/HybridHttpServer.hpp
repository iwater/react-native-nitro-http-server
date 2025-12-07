// cpp/HybridHttpServer.hpp
#pragma once
#include "../nitrogen/generated/shared/c++/HybridHttpServerSpec.hpp"

namespace margelo::nitro::http_server {

class HybridHttpServer : public HybridHttpServerSpec {
public:
    HybridHttpServer();
    ~HybridHttpServer() override;
    
    // 实现 HybridHttpServerSpec 接口
    std::shared_ptr<Promise<bool>> start(
        double port, 
        const std::function<std::shared_ptr<Promise<std::variant<HttpResponse, std::shared_ptr<Promise<HttpResponse>>>>>(const HttpRequest&)>& handler
    ) override;
    
    std::shared_ptr<Promise<bool>> sendResponse(
        const std::string& requestId, 
        const HttpResponse& response
    ) override;
    
    std::shared_ptr<Promise<void>> stop() override;
    
    std::shared_ptr<Promise<ServerStats>> getStats() override;
    
    std::shared_ptr<Promise<bool>> isRunning() override;
    
    // 静态服务器方法
    std::shared_ptr<Promise<bool>> startStaticServer(
        double port, 
        const std::string& rootDir
    ) override;
    
    std::shared_ptr<Promise<void>> stopStaticServer() override;

private:
    // 发送 HTTP 响应到 Rust 服务器的辅助方法
    void sendHttpResponse(const std::string& requestId, const HttpResponse& response);
};

} // namespace margelo::nitro::http_server
