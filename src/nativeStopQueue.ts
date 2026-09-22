/**
 * 原生 server 的 **stop → start 顺序闸**。
 *
 * 为什么需要它（2026-09-22 真机实测，RN 测试套件的 D8）：
 *
 * `stop()` 只是**发起**原生 stop 就返回；`'close'` 事件与 `close(cb)` 回调要等
 * **原生侧真的停完**才触发。在这中间调 `start()` 会与那次拆卸撞车 ——
 * 新 server 会正常报出一个端口，但**它的请求完全拿不到响应**
 * （客户端看到的是连接/读取失败，**不是** 500），而且**偶发**。
 * 真机上表现为「测试抖动」：同一个用例一轮过、一轮 `status=0`。
 *
 * Node 下 `server.close(); server.listen()` 是常规写法，不该有这种竞态，
 * 所以这里由模块自己把顺序管起来。
 *
 * ⚠️ 状态是**模块级**而不是实例字段：原生 server 是**单例**
 * （C++ 侧 `g_serverContext` / `g_serverRunning` / `g_serverPort`），
 * 所以 `serverA.close()` 之后 `serverB.start()` 同样会撞车 ——
 * 真机那次就是**两个不同实例**（Node 兼容层的 server2 → 工厂建的 s3）。
 */

let _pending: Promise<unknown> | null = null

/** 登记一次在飞的原生 stop。调用方仍然用自己那个 promise。 */
export function trackNativeStop(p: Promise<unknown>): void {
  _pending = p
  const clear = () => {
    if (_pending === p) _pending = null
  }
  // 成功/失败都要清，否则一次失败的 stop 会让后续 start 永远等下去
  p.then(clear, clear)
}

/**
 * 等所有在飞的原生 stop 落定。
 * stop **失败也放行** —— 失败的 stop 不该把 start 永久卡住。
 */
export async function awaitPendingNativeStop(): Promise<void> {
  while (_pending) {
    const p = _pending
    try {
      await p
    } catch (_) {
      /* stop 失败不阻塞 start */
    }
    if (_pending === p) _pending = null
  }
}

/**
 * stop 里用这个：登记 + 等它落定。
 * 与裸 `await` 的差别只在于**顺带登记**（供后续 start 等待）；
 * 失败仍然照原样抛出去，不吞。
 */
export async function stopWithTracking(p: Promise<unknown>): Promise<void> {
  trackNativeStop(p)
  await p
}
