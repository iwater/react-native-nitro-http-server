// cpp/RNHttpServerBackgroundTask.mm
// iOS 后台任务管理，延长 HTTP 服务器在锁屏后的存活时间

#import "RNHttpServerBackgroundTask.h"
#import <UIKit/UIKit.h>

// 全局回调指针
static void (*g_on_enter_background)(void) = NULL;
static void (*g_on_enter_foreground)(void) = NULL;

// 当前活跃的后台任务 ID
static UIBackgroundTaskIdentifier g_activeTaskId = UIBackgroundTaskInvalid;
static NSObject *g_observerToken1 = nil;
static NSObject *g_observerToken2 = nil;

unsigned long long rn_http_begin_background_task(void) {
    // 如果已经有一个活跃的任务，先结束它
    if (g_activeTaskId != UIBackgroundTaskInvalid) {
        [[UIApplication sharedApplication] endBackgroundTask:g_activeTaskId];
        g_activeTaskId = UIBackgroundTaskInvalid;
    }

    __block UIBackgroundTaskIdentifier taskId = UIBackgroundTaskInvalid;

    void (^startTask)(void) = ^{
        taskId = [[UIApplication sharedApplication]
            beginBackgroundTaskWithName:@"RNHttpServer"
                      expirationHandler:^{
                        // 后台时间即将耗尽，通知回调
                        if (g_on_enter_background) {
                            g_on_enter_background();
                        }
                        // 结束后台任务
                        [[UIApplication sharedApplication] endBackgroundTask:taskId];
                        g_activeTaskId = UIBackgroundTaskInvalid;
                      }];

        if (taskId != UIBackgroundTaskInvalid) {
            g_activeTaskId = taskId;
        }
    };

    if ([NSThread isMainThread]) {
        startTask();
    } else {
        dispatch_sync(dispatch_get_main_queue(), startTask);
    }

    return (unsigned long long)taskId;
}

void rn_http_end_background_task(unsigned long long taskId) {
    UIBackgroundTaskIdentifier identifier = (UIBackgroundTaskIdentifier)taskId;
    if (identifier != UIBackgroundTaskInvalid) {
        dispatch_async(dispatch_get_main_queue(), ^{
            [[UIApplication sharedApplication] endBackgroundTask:identifier];
            if (identifier == g_activeTaskId) {
                g_activeTaskId = UIBackgroundTaskInvalid;
            }
        });
    }
}

void rn_http_register_lifecycle_callbacks(
    void (*on_enter_background)(void),
    void (*on_enter_foreground)(void)
) {
    g_on_enter_background = on_enter_background;
    g_on_enter_foreground = on_enter_foreground;

    // 移除旧的观察者
    if (g_observerToken1) {
        [[NSNotificationCenter defaultCenter] removeObserver:g_observerToken1];
        g_observerToken1 = nil;
    }
    if (g_observerToken2) {
        [[NSNotificationCenter defaultCenter] removeObserver:g_observerToken2];
        g_observerToken2 = nil;
    }

    // 注册进入后台通知
    g_observerToken1 = [[NSNotificationCenter defaultCenter]
        addObserverForName:UIApplicationDidEnterBackgroundNotification
                    object:nil
                     queue:[NSOperationQueue mainQueue]
                usingBlock:^(NSNotification *note) {
                    // 进入后台时，启动一个后台任务来保活
                    rn_http_begin_background_task();

                    if (g_on_enter_background) {
                        g_on_enter_background();
                    }
                }];

    // 注册回到前台通知
    g_observerToken2 = [[NSNotificationCenter defaultCenter]
        addObserverForName:UIApplicationWillEnterForegroundNotification
                    object:nil
                     queue:[NSOperationQueue mainQueue]
                usingBlock:^(NSNotification *note) {
                    // 回到前台时，结束后台任务（不再需要）
                    if (g_activeTaskId != UIBackgroundTaskInvalid) {
                        [[UIApplication sharedApplication] endBackgroundTask:g_activeTaskId];
                        g_activeTaskId = UIBackgroundTaskInvalid;
                    }

                    if (g_on_enter_foreground) {
                        g_on_enter_foreground();
                    }
                }];
}
