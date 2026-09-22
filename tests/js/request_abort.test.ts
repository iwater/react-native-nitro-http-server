// 「客户端断开通知 JS」的 TS 层单测（requestAbort.ts + http.ts 的 Node 兼容层）
//
// 运行（在包根目录）：
//   node --import ./tests/js/register.mjs --test tests/js/request_abort.test.ts
//
// 覆盖边界：本文件测的是 **TS 层**。跨 FFI 那三段（Rust 判定 → C++ 回调 → Nitro 切线程）
// 在 qjs 探针里测，不在这里假装覆盖。
//
// ⚠️ 关于 `signal`：Node 22 **有** `AbortController`（RN 也有，见 setUpXHR.js 的
// polyfillGlobal），所以这里能测真实 signal；而 quickjs 的两个 headless runner
// **没有**（实测 typeof === 'undefined'）→ 探针里 signal 是 undefined。
// 两个宿主的行为差异已在 requestAbort.ts 的文件头写明。
//
// 有区分力的关键：断言的是「原生通知到达后，req/res 上的**可观测状态**」。
// 把 `abortEntry.listeners.add(...)` 那一行摘掉，C1–C4 必须全红。

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { __setHybridObjectFactory } from 'react-native-nitro-modules';
import { createServer } from '../../src/http.ts';
import * as ra from '../../src/requestAbort.ts';

// ---------------------------------------------------------------------------
// 原生侧替身：把 setRequestAbortedHandler 装进来的回调抓下来，供用例手动驱动
// ---------------------------------------------------------------------------

function createFakeNative() {
  let abortHandler: ((requestId: string) => void) | null = null;
  let installCount = 0;
  const startArgs: unknown[][] = [];
  const nextPort = 19999;

  const fake = {
    start: (...args: unknown[]) => {
      startArgs.push(args);
      return Promise.resolve(nextPort);
    },
    stop: () => Promise.resolve(undefined),
    readRequestBodyChunk: () => Promise.resolve(''),
    writeResponseChunk: () => Promise.resolve(true),
    endResponse: () => Promise.resolve(true),
    sendBinaryResponse: () => Promise.resolve(true),
    setRequestAbortedHandler: (h: (requestId: string) => void) => {
      installCount += 1;
      abortHandler = h;
    },
  };

  return {
    fake,
    get abortHandler() {
      assert.ok(abortHandler, '原生侧还没有装请求中断回调');
      return abortHandler!;
    },
    get installCount() {
      return installCount;
    },
    get startArgs() {
      return startArgs;
    },
    reset() {
      installCount = 0;
      startArgs.length = 0;
    },
  };
}

const harness = createFakeNative();
__setHybridObjectFactory(() => harness.fake);

// ⚠️ index.ts 在**模块顶层**就 createHybridObject('HttpServer')，静态 import 会被提升
// 到模块体之前（工厂还没装）→ 必须动态 import。C7 用它。
const mod: typeof import('../../src/index.ts') = await import('../../src/index.ts');

// ---------------------------------------------------------------------------
// A. requestAbort 本体
// ---------------------------------------------------------------------------

beforeEach(() => {
  ra.__resetRequestAbortForTests();
  harness.reset();
});

test('A1 trackRequest 给出真实 AbortSignal（宿主有 AbortController 时）', () => {
  assert.equal(
    typeof AbortController,
    'function',
    'Node 22 应当有 AbortController —— 没有的话本文件的多条断言会退化成无效判据'
  );
  const entry = ra.trackRequest('r-a1');
  assert.ok(entry.signal, 'signal 应当存在');
  assert.equal(entry.signal!.aborted, false);
  assert.equal(ra.pendingRequestCount(), 1);
});

test('A2 onRequestAborted → signal 被 abort 且触发 abort 事件', () => {
  const entry = ra.trackRequest('r-a2');
  const hits: string[] = [];
  entry.signal!.addEventListener('abort', () => hits.push('signal:abort'));

  ra.onRequestAborted('r-a2');

  assert.equal(entry.signal!.aborted, true, 'signal 应当已 abort');
  assert.deepEqual(hits, ['signal:abort']);
  assert.equal(entry.aborted, true);
});

test('A3 通知之后登记被摘掉（条目是一次性的）', () => {
  ra.trackRequest('r-a3');
  assert.equal(ra.pendingRequestCount(), 1);
  ra.onRequestAborted('r-a3');
  assert.equal(ra.pendingRequestCount(), 0, '通知后不应残留登记');
});

test('A4 重复通知是幂等的（signal 事件只发一次）', () => {
  const entry = ra.trackRequest('r-a4');
  const hits: string[] = [];
  entry.signal!.addEventListener('abort', () => hits.push('signal:abort'));

  ra.onRequestAborted('r-a4');
  ra.onRequestAborted('r-a4');
  ra.onRequestAborted('r-a4');

  assert.deepEqual(hits, ['signal:abort']);
});

test('A5 未知 requestId 的通知不抛（竞态兜底）', () => {
  assert.doesNotThrow(() => ra.onRequestAborted('never-tracked'));
});

test('A6 已摘登记之后的通知是 no-op —— 即「正常回完不会误报中断」', () => {
  const entry = ra.trackRequest('r-a6');
  ra.untrackRequest('r-a6');
  ra.onRequestAborted('r-a6');
  assert.equal(entry.signal!.aborted, false, '摘掉之后不应再被 abort');
});

test('A7 同一 requestId 重复登记返回同一条目', () => {
  const a = ra.trackRequest('r-a7');
  const b = ra.trackRequest('r-a7');
  assert.equal(a, b);
  assert.equal(ra.pendingRequestCount(), 1);
});

test('A8 listeners 在 signal abort 之前跑（两个入口看到的状态一致）', () => {
  const entry = ra.trackRequest('r-a8');
  const order: string[] = [];
  entry.listeners.add(() => order.push('listener'));
  entry.signal!.addEventListener('abort', () => order.push('signal:abort'));

  ra.onRequestAborted('r-a8');

  assert.deepEqual(order, ['listener', 'signal:abort']);
});

test('A9 单个 listener 抛错不影响其余 listener 与 signal', () => {
  const entry = ra.trackRequest('r-a9');
  const ran: string[] = [];
  entry.listeners.add(() => {
    throw new Error('boom');
  });
  entry.listeners.add(() => ran.push('second'));

  const origError = console.error;
  console.error = () => {};
  try {
    ra.onRequestAborted('r-a9');
  } finally {
    console.error = origError;
  }

  assert.deepEqual(ran, ['second']);
  assert.equal(entry.signal!.aborted, true);
});

test('B1 installRequestAbortedHandler 是幂等的（原生只被装一次）', () => {
  ra.installRequestAbortedHandler(harness.fake as never);
  ra.installRequestAbortedHandler(harness.fake as never);
  ra.installRequestAbortedHandler(harness.fake as never);
  assert.equal(harness.installCount, 1);
});

test('B2 装进原生的回调驱动的是同一条链路', () => {
  ra.installRequestAbortedHandler(harness.fake as never);
  const entry = ra.trackRequest('r-b2');
  harness.abortHandler('r-b2');
  assert.equal(entry.signal!.aborted, true);
});

// ---------------------------------------------------------------------------
// C. http.ts 的 Node 兼容层集成
// ---------------------------------------------------------------------------

interface Captured {
  req: any;
  res: any;
}

/**
 * 起一个 Server（桩原生），拿到它交给原生的 handler。
 * 同一个 server 可以 dispatch 多条请求（`listen` 只能调一次）。
 */
async function startServer(server: ReturnType<typeof createServer>) {
  const captured: Captured[] = [];
  server.on('request', (req: any, res: any) => {
    captured.push({ req, res });
  });

  server.listen(0);
  // listen 是异步的（等原生 start 落定）
  await new Promise((r) => setTimeout(r, 0));

  const last = harness.startArgs[harness.startArgs.length - 1];
  assert.ok(last, '原生 start 未被调用');
  const nativeHandler = last![1] as (request: unknown) => Promise<unknown>;

  return {
    dispatch(requestId: string): Captured {
      // 不 await：handler 要一直挂着（模拟「还在处理，客户端走了」）
      void nativeHandler({
        requestId,
        method: 'POST',
        path: '/slow',
        headers: { 'content-type': 'text/plain' },
      });
      const hit = captured[captured.length - 1];
      assert.ok(hit, "没有拿到 req/res（'request' 事件没触发）");
      return hit!;
    },
  };
}

/** 起一个 server 并驱动一条请求 */
async function startAndDispatch(server: ReturnType<typeof createServer>, requestId: string): Promise<Captured> {
  const h = await startServer(server);
  return h.dispatch(requestId);
}

test('C1 中断通知 → req.aborted / res.destroyed 置位', async () => {
  const server = createServer(() => {
    /* 故意不回，模拟 handler 挂起 */
  });
  const { req, res } = await startAndDispatch(server, 'c1');

  assert.equal(req.aborted, false, '中断前应为 false');
  assert.equal(res.destroyed, false, '中断前应为 false');

  harness.abortHandler('c1');

  assert.equal(req.aborted, true);
  assert.equal(res.destroyed, true);
});

test('C2 req 收到 aborted 与 close 事件（顺序同 Node 实测）', async () => {
  const server = createServer(() => {});
  const { req } = await startAndDispatch(server, 'c2');

  const events: string[] = [];
  req.on('aborted', () => events.push('aborted'));
  req.on('close', () => events.push('close'));

  harness.abortHandler('c2');
  assert.deepEqual(events, ['aborted', 'close']);
});

test('C3 res 收到 close 事件，且 writableEnded 保持 false（同 Node 实测）', async () => {
  const server = createServer(() => {});
  const { res } = await startAndDispatch(server, 'c3');

  const events: string[] = [];
  res.on('close', () => events.push('close'));

  harness.abortHandler('c3');
  assert.deepEqual(events, ['close']);
  assert.equal(res.writableEnded, false, 'Node 断开后 writableEnded 仍是 false');
  assert.equal(res.writable, false);
});

test('C4 有 error 监听器才发 ECONNRESET；没监听器时完全不发（同 Node 实测）', async () => {
  // 观测手法：把 emit 换成记录器 —— 比「挂/不挂监听器」直接，且能同时断言事件顺序。
  // ⚠️ 不能靠「没挂监听器会不会抛」来判定：本模块的 EventEmitter 是 eventemitter3，
  // 它对未处理的 'error' **不抛**（实测），那条断言在有无 guard 时都是绿的（没牙齿）。
  const spy = (target: any) => {
    const seen: string[] = [];
    const orig = target.emit.bind(target);
    target.emit = (ev: string, ...args: any[]) => {
      seen.push(ev);
      return orig(ev, ...args);
    };
    return seen;
  };

  // ① 没有 'error' 监听器 —— 不得发 'error'
  {
    const server = createServer(() => {});
    const { req } = await startAndDispatch(server, 'c4a');
    const seen = spy(req);
    harness.abortHandler('c4a');
    assert.deepEqual(seen, ['aborted', 'close'], "无监听器时不该出现 'error'");
  }

  // ② 挂了 'error' 监听器 —— 必须在 aborted 之后、close 之前收到 ECONNRESET
  {
    const server = createServer(() => {});
    const { req } = await startAndDispatch(server, 'c4b');
    const codes: string[] = [];
    req.on('error', (e: any) => codes.push(e.code));
    const seen = spy(req);
    harness.abortHandler('c4b');
    assert.deepEqual(codes, ['ECONNRESET']);
    assert.deepEqual(seen, ['aborted', 'error', 'close']);
  }
});

test('C5 handler 正常响应 → 登记被摘，之后的通知是 no-op', async () => {
  const server = createServer((_req, res) => {
    res.end('ok');
  });
  await startAndDispatch(server, 'c5');

  // 让 res.end() 的串行链落地
  await new Promise((r) => setTimeout(r, 20));

  assert.equal(ra.pendingRequestCount(), 0, '正常回完不应残留登记');
  assert.doesNotThrow(() => harness.abortHandler('c5'));
});

test('C6 中断只影响对应 requestId，不误伤其他在飞请求', async () => {
  const server = createServer(() => {});
  const h = await startServer(server);
  const a = h.dispatch('c6-a');
  const b = h.dispatch('c6-b');

  harness.abortHandler('c6-a');

  assert.equal(a.req.aborted, true);
  assert.equal(b.req.aborted, false, 'b 不该被牵连');
  assert.equal(b.res.destroyed, false, 'b 不该被牵连');
});

test('C7 高层 handler（index.ts 的 wrapHandler）拿到的 request 带 signal，中断时被 abort', async () => {
  const server = new mod.HttpServer();
  let captured: any = null;
  await server.start(0, (req: any) => {
    captured = req;
    return new Promise(() => {
      /* 永不返回：模拟 handler 挂起中客户端走了 */
    });
  });

  // 桩原生不会自己调 handler，手动驱动一次
  const last = harness.startArgs[harness.startArgs.length - 1];
  const wrapped = last![1] as (r: unknown) => Promise<unknown>;
  void wrapped({ requestId: 'c7', method: 'GET', path: '/slow', headers: {} });

  assert.ok(captured, '用户 handler 未被调用');
  assert.ok(captured.signal, 'request 上应当挂上 signal');
  assert.equal(captured.signal.aborted, false, '中断前不应 aborted');

  harness.abortHandler('c7');

  assert.equal(captured.signal!.aborted, true, '中断后 signal 应当 aborted');
  assert.equal(ra.pendingRequestCount(), 0, '中断后登记应被摘掉');
});
