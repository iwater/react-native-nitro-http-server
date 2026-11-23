
// wrapper.h
#ifndef WRAPPER_H
#define WRAPPER_H

#include <stdbool.h>
#include <stdint.h>

// 请求数据结构
typedef struct HttpRequest {
    char* request_id;
    char* method;
    char* path;
    char* headers_json;
    char* body;
    int body_len;
} HttpRequest;

// 回调函数类型
typedef void (*RequestCallback)(HttpRequest*);

// 函数声明
// 注意：start_* 四个函数返回的是**实际端口号**（>0 成功，<=0 失败），不是 bool——
// 与 src/c_api.rs 的 c_int 返回值一致。若错写成 bool，C 调用方在 -O2/-Os 下会被
// 优化器按「bool ∈ {0,1}」的域假设常量折叠（实测 g_serverPort 被存成 1）。
int start_server(int port, const char* host, RequestCallback callback);
int start_server_with_options(int port, const char* host, RequestCallback callback, int worker_threads);
bool send_response(const char* request_id, int status_code, 
                   const char* headers_json, const char* body, int body_len);
void stop_server();
const char* get_server_stats();
void free_http_request(HttpRequest* request);
void free_string(char* s);
int start_static_server(int port, const char* host, const char* root_dir);
void stop_static_server();
// 启动 App 服务器
int start_app_server(int port, const char* host, const char* root_dir, RequestCallback callback);
void stop_app_server();
// 启动 App 服务器 (带配置)
int start_server_with_config(int port, const char* host, RequestCallback callback, const char* config_json);
// 流式 API
int read_request_body_chunk(const char* request_id, char* buffer, int buffer_size);
bool write_response_chunk(const char* request_id, const char* chunk, int chunk_len);
bool end_response(const char* request_id, int status_code, const char* headers_json);

// CORS 配置
void set_cors_config(const char* config_json);

#endif // WRAPPER_H
