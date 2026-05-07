// cpp/RNHttpServerBackgroundTask.h
#pragma once

#ifdef __cplusplus
extern "C" {
#endif

/// 开始一个后台任务，以防止 iOS 在锁屏后立即挂起 HTTP 服务器
/// 必须在主线程上调用
/// 返回后台任务标识符（0 表示失败 / UIBackgroundTaskInvalid）
unsigned long long rn_http_begin_background_task(void);

/// 结束后台任务
void rn_http_end_background_task(unsigned long long taskId);

/// 注册前后台切换回调
/// on_enter_background: App 进入后台时回调
/// on_enter_foreground: App 回到前台时回调
void rn_http_register_lifecycle_callbacks(
    void (*on_enter_background)(void),
    void (*on_enter_foreground)(void)
);

#ifdef __cplusplus
}
#endif
