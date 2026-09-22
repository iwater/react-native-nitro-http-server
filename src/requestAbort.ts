/**
 * 「请求被客户端中断」的通知中枢。
 *
 * 链路：Rust 的 `RequestStateGuard::drop` 判定「响应从未发出」（= 客户端断开、
 * hyper 把 handler future 丢了）→ 调 C 回调 → C++ 的 `c_request_aborted_callback`
 * → Nitro 的 `AsyncJSCallback` 自动切回 JS 线程 → 本模块的处理器。
 *
 * 为什么需要一张表：原生回调只给 `requestId`，而 JS 侧要把通知送到**具体的
 * request / response 对象**上（`req.aborted` / `req.emit('aborted')` / `res.destroyed`），
 * 以及一个 `AbortSignal`（供 handler 里 `await fetch(url, { signal })` 之类使用）。
 *
 * ⚠️ `AbortController` 由**宿主**提供：
 *   - React Native 有（`Libraries/Core/setUpXHR.js` 里 `polyfillGlobal('AbortController', …)`）；
 *   - 无头 JS 宿主一般没有 —— 实测 `typeof globalThis.AbortController === 'undefined'`
 *     （本模块的无头端到端验证就跑在这样一个宿主上）。
 * 缺失时 `signal` 就是 `undefined`，其余中断通知照常工作。这里刻意**不做**内部兜底
 * 实现 —— 一个只在测试宿主上跑得起来的自制 signal，会让用例验的不是真东西。
 */

import type { HttpServer as NitroHttpServer } from './HttpServer.nitro'

/** 一个在飞请求的中断状态 */
export interface RequestAbortEntry {
    /** 客户端断开时被 abort；宿主没有 AbortController 时为 null */
    controller: AbortController | null
    /** 宿主支持时 = `controller.signal`；否则 undefined（见文件头） */
    signal: AbortSignal | undefined
    /**
     * Node 兼容层注册的额外中断回调（`http.ts` 用它置 `req.aborted` /
     * `res.destroyed` 并发 `'aborted'` 事件）。高层的 `wrapHandler` 不注册。
     */
    listeners: Set<() => void>
    /** 已经收到过中断通知（幂等保护） */
    aborted: boolean
}

/** 宿主是否提供 AbortController（只探一次） */
const HAS_ABORT_CONTROLLER = typeof AbortController !== 'undefined'

/**
 * 在飞请求表。
 *
 * 条目在**两个**时机被摘掉，任一个先到即可：
 *  1. 收到中断通知时（原生侧的 guard 每个请求只 drop 一次 → 通知是一次性的）；
 *  2. handler 落定（正常返回 / 抛错）时。
 *
 * 残留的唯一情形是「handler 永不 settle 且客户端不中断」—— 与 Node 里
 * 「handler 挂着不返回」同类，每个这样的请求残留一条，不做额外兜底。
 */
const pendingRequests = new Map<string, RequestAbortEntry>()

function createEntry(): RequestAbortEntry {
    if (!HAS_ABORT_CONTROLLER) {
        return { controller: null, signal: undefined, listeners: new Set(), aborted: false }
    }
    const controller = new AbortController()
    return { controller, signal: controller.signal, listeners: new Set(), aborted: false }
}

/**
 * 登记一个在飞请求。同一个 requestId 重复登记返回**同一个**条目
 * （`ServerResponse` 与 `wrapHandler` 可能都想登记）。
 */
export function trackRequest(requestId: string): RequestAbortEntry {
    const existing = pendingRequests.get(requestId)
    if (existing) return existing
    const entry = createEntry()
    pendingRequests.set(requestId, entry)
    return entry
}

/** 摘掉登记（幂等；不存在也不报错） */
export function untrackRequest(requestId: string): void {
    pendingRequests.delete(requestId)
}

/** 当前登记数。诊断 / 泄漏断言用。 */
export function pendingRequestCount(): number {
    return pendingRequests.size
}

/**
 * 原生「客户端断开」通知的落点。**由 `installRequestAbortedHandler` 装给原生侧。**
 *
 * 顺序刻意是「先跑 listeners，再 abort signal」：listeners 负责置
 * `req.aborted` / `res.destroyed`，这样用户挂在 `signal` 上的监听器跑起来时，
 * 这两个标志已经是 `true`（两个入口看到的状态一致）。
 */
export function onRequestAborted(requestId: string): void {
    const entry = pendingRequests.get(requestId)
    if (!entry) {
        // 请求已经收尾（正常路径本来就不会有通知）—— 竞态兜底，静默忽略
        return
    }
    if (entry.aborted) return
    entry.aborted = true

    // 通知是一次性的：先摘表，避免 handler 永不 settle 时把条目一直留着
    pendingRequests.delete(requestId)

    for (const listener of Array.from(entry.listeners)) {
        try {
            listener()
        } catch (e) {
            console.error('[http-server] request-abort listener threw:', e)
        }
    }

    if (entry.controller) {
        try {
            entry.controller.abort()
        } catch (e) {
            console.error('[http-server] AbortController.abort() threw:', e)
        }
    }
}

/**
 * 是否已经装过原生回调。
 *
 * 原生侧（`cpp/HybridHttpServer.cpp` 的 `g_abortHandler`）与 `setWebSocketHandler`
 * 一样是**全局单回调、后装者胜**，而本模块有多个 server 类型 —— 装一次就够，
 * 后装的会把同一份实现再装一遍（无害，但没必要）。
 */
let installed = false

/**
 * 把本模块的处理器装进原生侧。**幂等**，可以随便多调。
 *
 * ⚠️ 刻意**不**在 `index.ts` 模块顶层调用：那会让「只 import 不 start」的用例
 * （如 `http_write_order.test.ts`）也必须提供桩方法。这里放在 `wrapHandler` /
 * `Server.listen()` 里 —— 都在真正要起服务的那一刻。
 */
export function installRequestAbortedHandler(module: NitroHttpServer): void {
    if (installed) return
    installed = true
    module.setRequestAbortedHandler(onRequestAborted)
}

/** 仅测试用：复位安装标记与登记表（同进程内重跑用例时用） */
export function __resetRequestAbortForTests(): void {
    installed = false
    pendingRequests.clear()
}
