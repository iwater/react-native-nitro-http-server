
// wrapper.h
#ifndef WRAPPER_H
#define WRAPPER_H

#include <stdbool.h>
#include <stdint.h>

typedef struct HttpRequest {
    char* request_id;
    char* method;
    char* path;
    char* headers_json;
    char* body;
    int body_len;
} HttpRequest;

typedef void (*RequestCallback)(HttpRequest*);

// Note: start_* functions return the actual bound port (>0 on success, <=0 on failure),
// not bool, matching the c_int return type in src/c_api.rs.
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
int start_app_server(int port, const char* host, const char* root_dir, RequestCallback callback);
void stop_app_server();
int start_server_with_config(int port, const char* host, RequestCallback callback, const char* config_json);
int read_request_body_chunk(const char* request_id, char* buffer, int buffer_size);
bool write_response_chunk(const char* request_id, const char* chunk, int chunk_len);
bool end_response(const char* request_id, int status_code, const char* headers_json);

void set_cors_config(const char* config_json);

#endif // WRAPPER_H
