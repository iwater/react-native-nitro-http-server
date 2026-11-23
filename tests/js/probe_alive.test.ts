// Task 12：`_probeAlive` 健壮性（超时 20ms → 500ms；失败 100ms 后重试一次才判死）
//
// 跑法（包根目录）：npm run test:js
//
// 为什么表驱动跑 **5 个类**，而不是只测其中一份
// ------------------------------------------
// `_probeAlive` 在源码里有 **5 份逐字相同** 的实现（去掉注释后 sha 相同）：
//   grep -n "_probeAlive" src/  →
//     index.ts:130 HttpServer / :236 StaticServer / :339 AppServer / :586 ConfigServer
//     http.ts:740  Server
// 计划要求「5 处统一改为」。只测一份的话，另外 4 份漂移了看不出来 ——
// 而它们各自服务不同的类，漂移后症状只会在对应那类上出现。
//
// 桩 fetch 必须**尊重 AbortController**（区分力的来源）
// ------------------------------------------------
// 旧实现 20ms 就 abort。如果桩忽略 signal 而是「等 150ms 后 resolve」，
// 旧实现也会拿到 true → 测试全绿却什么都没测到。所以桩在收到 abort 时必须
// reject（与真实 fetch 一致）。
//
// 「不算死」这件事的真实症状是**重启健康服务器**（重启窗口内连接全断），
// 所以除了直接测 `_probeAlive()`，还从 AppState 入口做了一条集成用例。

import { test, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';

// 由 tests/js/stubs-loader.mjs 重定向到 tests/js/stubs/
import { __setHybridObjectFactory } from 'react-native-nitro-modules';
import { __emitAppState, __resetAppState } from 'react-native';

const PORT = 18099;

// ---------------------------------------------------------------------------
// 原生侧替身（只需要 start/stop 那几个方法）
// ---------------------------------------------------------------------------
function createFakeNative() {
  const calls: string[] = [];
  const rec = (name: string, value: unknown) => () => {
    calls.push(name);
    return Promise.resolve(value);
  };
  const fake = {
    start: rec('start', PORT),
    startServerWithConfig: rec('startServerWithConfig', PORT),
    stop: rec('stop', undefined),
    stopAppServer: rec('stopAppServer', undefined),
    isRunning: rec('isRunning', true),
    getStats: rec('getStats', {}),
    sendBinaryResponse: rec('sendBinaryResponse', true),
    wsSendText: rec('wsSendText', true),
    wsSendBinary: rec('wsSendBinary', true),
    wsClose: rec('wsClose', true),
    setCorsConfig: () => { calls.push('setCorsConfig'); },
    setWebSocketHandler: () => { calls.push('setWebSocketHandler'); },
    setRequestAbortedHandler: () => { calls.push('setRequestAbortedHandler'); },
  };
  return {
    fake,
    calls,
    count(name: string) {
      return calls.filter((c) => c === name).length;
    },
    reset() {
      calls.length = 0;
    },
  };
}

const harness = createFakeNative();
__setHybridObjectFactory(() => harness.fake);

const mod: typeof import('../../src/index.ts') = await import('../../src/index.ts');
const httpMod: typeof import('../../src/http.ts') = await import('../../src/http.ts');

/** 5 个类各有一份 `_probeAlive`，逐个跑 */
function probeCases(): Array<[string, () => { _port: number; _probeAlive: () => Promise<boolean> }]> {
  return [
    ['HttpServer (index.ts:130)', () => new mod.HttpServer()],
    ['StaticServer (index.ts:236)', () => new mod.StaticServer()],
    ['AppServer (index.ts:339)', () => new mod.AppServer()],
    ['ConfigServer (index.ts:586)', () => new mod.ConfigServer()],
    ['Server (http.ts:740)', () => new httpMod.Server()],
  ];
}

// ---------------------------------------------------------------------------
// fetch 桩：尊重 AbortController
// ---------------------------------------------------------------------------
interface FetchPlan {
  /** 成功路径的耗时（ms）；0 = 立即成功 */
  delayMs?: number;
  /** 前 N 次直接失败（模拟 ECONNREFUSED，立即 reject） */
  failFirst?: number;
  /** 一直失败 */
  failAlways?: boolean;
}

function stubFetch(plan: FetchPlan) {
  const orig = globalThis.fetch;
  const attempts: number[] = [];
  let n = 0;

  globalThis.fetch = ((_url: string, init: { signal?: AbortSignal }) => {
    n += 1;
    attempts.push(Date.now());
    const thisCall = n;
    const shouldFail = plan.failAlways || (plan.failFirst !== undefined && thisCall <= plan.failFirst);

    return new Promise((resolve, reject) => {
      if (shouldFail) {
        reject(new Error('connect ECONNREFUSED 127.0.0.1:1'));
        return;
      }
      const timer = setTimeout(() => resolve({ ok: true, status: 200 }), plan.delayMs ?? 0);
      const signal = init && init.signal;
      if (signal) {
        const onAbort = () => {
          clearTimeout(timer);
          reject(new Error('AbortError: The operation was aborted.'));
        };
        if (signal.aborted) onAbort();
        else signal.addEventListener('abort', onAbort);
      }
    });
  }) as typeof fetch;

  return {
    attempts,
    restore() {
      globalThis.fetch = orig;
    },
  };
}

/** 进程内还挂着的定时器数（用来发现没被 clearTimeout 掉的 abort 定时器） */
const pendingTimers = () => process.getActiveResourcesInfo().filter((r) => r === 'Timeout').length;

let stub: ReturnType<typeof stubFetch> | null = null;

beforeEach(() => {
  harness.reset();
  __resetAppState();
});

afterEach(() => {
  if (stub) {
    stub.restore();
    stub = null;
  }
});

// ===========================================================================
// 1. 慢响应不该被判死（核心修复）
// ===========================================================================

for (const [name, make] of probeCases()) {
  test(`本地往返 150ms 不算死：${name}`, async () => {
    stub = stubFetch({ delayMs: 150 });
    const inst = make();
    inst._port = PORT;

    assert.equal(
      await inst._probeAlive(),
      true,
      '150ms 的本地往返是正常的（JS 线程忙时更久）。判死会去重启一个健康服务器，' +
        '而重启窗口内所有连接都会断 —— 这比不重启严重得多'
    );
  });
}

// ===========================================================================
// 2. 失败后重试一次
// ===========================================================================

test('首次探测失败、重试成功 → 判活（且恰好探测 2 次）', async () => {
  stub = stubFetch({ failFirst: 1, delayMs: 0 });
  const inst = new mod.ConfigServer();
  inst._port = PORT;

  assert.equal(await inst._probeAlive(), true);
  assert.equal(stub.attempts.length, 2, '第一次失败后必须再试一次');
});

test('两次都失败 → 才判死（且恰好探测 2 次）', async () => {
  stub = stubFetch({ failAlways: true });
  const inst = new mod.ConfigServer();
  inst._port = PORT;

  assert.equal(await inst._probeAlive(), false);
  assert.equal(stub.attempts.length, 2, '不能只探测一次就判死');
});

test('首次就成功 → 不重复探测（只 1 次）', async () => {
  stub = stubFetch({ delayMs: 0 });
  const inst = new mod.ConfigServer();
  inst._port = PORT;

  assert.equal(await inst._probeAlive(), true);
  assert.equal(stub.attempts.length, 1, '成功路径不该有多余的探测');
});

test('重试前有等待（不是立刻连打两次）', async () => {
  stub = stubFetch({ failAlways: true });
  const inst = new mod.ConfigServer();
  inst._port = PORT;

  await inst._probeAlive();

  assert.equal(stub.attempts.length, 2);
  const gap = stub.attempts[1] - stub.attempts[0];
  // 只断言下界：上界依赖机器负载，断言它就是弱测试（判定的是"机器快"而不是"有延迟"）
  assert.ok(gap >= 90, `两次探测之间应等 ~100ms，实测 ${gap}ms`);
});

// ===========================================================================
// 3. abort 定时器必须被清掉
// ===========================================================================

test('探测失败不留悬挂的 abort 定时器', async () => {
  stub = stubFetch({ failAlways: true });
  const inst = new mod.ConfigServer();
  inst._port = PORT;

  const before = pendingTimers();
  await inst._probeAlive();
  const after = pendingTimers();

  assert.equal(
    after - before,
    0,
    '每次探测都挂一个 setTimeout(abort, 500)，失败路径不 clearTimeout 就会留下 2 个 ' +
      '500ms 的悬挂定时器（重试一次 = 两个），既拖住事件循环也让「探测次数」统计失真'
  );
});

// ===========================================================================
// 4. 集成：慢响应下不该触发重启（用户可见的症状）
// ===========================================================================

test('AppState 回到前台 + 服务器只是慢（150ms）→ 不得重启', async () => {
  stub = stubFetch({ delayMs: 150 });
  const server = new mod.ConfigServer();
  await server.start(0, () => ({ statusCode: 200, headers: {}, body: 'ok' }), {
    mounts: [],
  }, { host: '127.0.0.1', autoRestart: true });

  harness.reset();
  await __emitAppState('active');

  assert.equal(
    harness.count('stopAppServer') + harness.count('startServerWithConfig'),
    0,
    '服务器只是慢，不是死了 —— 重启会把所有连接打断'
  );

  await server.stop();
});

test('AppState 回到前台 + 服务器真的死了 → 仍然会重启（别把这条修成"永不重启"）', async () => {
  stub = stubFetch({ failAlways: true });
  const server = new mod.ConfigServer();
  await server.start(0, () => ({ statusCode: 200, headers: {}, body: 'ok' }), {
    mounts: [],
  }, { host: '127.0.0.1', autoRestart: true });

  harness.reset();
  await __emitAppState('active');

  assert.equal(harness.count('stopAppServer'), 1, '真死了要停旧的');
  assert.equal(harness.count('startServerWithConfig'), 1, '真死了要起新的');

  await server.stop();
});
