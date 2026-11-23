// 「请求型」loop-ref 契约：不持句柄的异步原生方法必须「发起时 acquire、settle 时 release」
//
// 跑法（包根目录）：npm run test:js
//
// 【这条契约解决什么】
// 本库把 Node 的 handle-ref 模型桥到**宿主能力** `globalThis.__qjsHandleRef`
// （见 src/loopRef.ts：宿主提供它，RN 宿主不提供 → 全部 no-op，行为零变化）。
// 该模型有两半，缺任何一半都会得到「有些异步调用能用、有些永远不 settle」：
//
//   - **handle 型**：句柄建立时 acquire、关闭成功后 release（失败路径也要放掉）。
//     running 的 server 自己顶住 loop —— 由 listen/close 与回归用例覆盖。
//   - **请求型**：**发起时 acquire、settle 时 release**。一次性异步方法没有句柄可挂；
//     不登记的话，当它是**程序最后一个动作**时，宿主会先判定「无事可做」而收泵退出，
//     原生结果随后回来时无处投递 → **Promise 永不 settle**（进程静默退出，无任何报错）。
//     本文件钉的就是这一半。
//
// 【为什么这条用例是确定性的】
// 不真跑异步、不 sleep、不看概率：装一个**探针宿主**记录 acquire/release，
// 并让原生替身返回一个**由本用例控制的 pending promise**，
// 于是「调用在途」这一刻可以**同步**读出 ref 持有数。
// 修复前必然红（acquire 从未发生），修复后必然绿 —— 没有中间态、不会抖动。
// 这与「最后一个动作是异步调用，看它到底跑没跑完」那种**概率性**用例互补：
// 后者会因时序而假绿（实测同一用例 Debug 12/15、Release 3/15），不能当唯一守卫。
//
// 【为什么表驱动覆盖 4 个类】
// `isRunning()` 在源码里有 4 份同形实现（HttpServer / StaticServer / AppServer /
// ConfigServer），只测一份的话另外 3 份漂移了看不出来 —— 而它们各自服务不同的类。
//
// ⚠️ 【桩的稳定性要求（踩过）】
// `src/index.ts` 在**模块顶层**只 `createHybridObject('HttpServer')` 一次，
// 而 ESM 模块只求值一次 → **换工厂返回的对象不会生效**：后续用例里模块仍握着
// 第一个用例的那个替身。所以替身必须是**跨用例稳定的同一个对象**，
// 「每用例一份可控行为」要放在它委托的**控制器**上（下面的 `ctl`）。
// 反过来做（每个 beforeEach 新建 fake）的症状是：第 1 个用例通过、其余全部
// 报「原生方法还没被调用」——看起来像被测代码没调原生，其实是桩绑错了。

import { test, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';

// 由 tests/js/stubs-loader.mjs 重定向到 tests/js/stubs/
import { __setHybridObjectFactory, __clearHybridObjectFactory } from 'react-native-nitro-modules';
// 与 src/index.ts 里的 `import { LoopRef } from './loopRef'` 解析到**同一个模块实例**
// （同一个文件 URL），所以这里装的探针宿主就是 LoopRef 会读到的那个。
import { __setHostHandleRefForTest } from '../../src/loopRef';

// ---------------------------------------------------------------------------
// 控制器：**每用例一份**的可控行为。稳定的替身对象委托给它。
// ---------------------------------------------------------------------------
interface NativeControl {
  isRunning: () => Promise<boolean>;
  getStats: () => Promise<Record<string, unknown>>;
}

let ctl: NativeControl = {
  isRunning: () => Promise.resolve(false),
  getStats: () => Promise.resolve({}),
};

// ---------------------------------------------------------------------------
// 原生替身：**跨用例稳定**（见文件头「桩的稳定性要求」）
// ---------------------------------------------------------------------------
const fakeNative = {
  isRunning: () => ctl.isRunning(),
  getStats: () => ctl.getStats(),
  // 其余方法本用例不碰；给立即 resolve 的桩，避免构造期或别处被调到时报错。
  start: () => Promise.resolve(0),
  stop: () => Promise.resolve(),
  startStaticServer: () => Promise.resolve(0),
  stopStaticServer: () => Promise.resolve(),
  startAppServer: () => Promise.resolve(0),
  stopAppServer: () => Promise.resolve(),
  startServerWithConfig: () => Promise.resolve(0),
  sendBinaryResponse: () => Promise.resolve(true),
  wsSendText: () => Promise.resolve(true),
  wsSendBinary: () => Promise.resolve(true),
  wsClose: () => Promise.resolve(true),
  setCorsConfig: () => {},
  setWebSocketHandler: () => {},
  setRequestAbortedHandler: () => {},
};

__setHybridObjectFactory(() => fakeNative);

// ---------------------------------------------------------------------------
// 受控的原生调用：返回一个由用例显式 settle 的 pending promise
// ---------------------------------------------------------------------------
function controllableIsRunning() {
  let settle: { resolve: (v: boolean) => void; reject: (e: Error) => void } | null = null;
  const promise = new Promise<boolean>((resolve, reject) => {
    settle = { resolve, reject };
  });
  const api = {
    /** 用例读它来判断「原生到底被调用了没有」 */
    get called(): boolean {
      return settle !== null;
    },
    resolve(v: boolean) {
      assert.ok(settle, 'isRunning() 还没被调用，无法 settle');
      settle!.resolve(v);
    },
    reject(e: Error) {
      assert.ok(settle, 'isRunning() 还没被调用，无法 settle');
      settle!.reject(e);
    },
  };
  return { promise, api };
}

// ---------------------------------------------------------------------------
// 探针宿主：记录每一次 acquire/release，并给出当前持有数
// ---------------------------------------------------------------------------
function createProbeHost() {
  let nextToken = 1;
  const live = new Set<number>();
  const log: string[] = [];
  return {
    log,
    /** 当前被持有的 ref 数 */
    get held(): number {
      return live.size;
    },
    /** 累计 acquire 次数（用于排除「失败路径上断言空转」） */
    get acquires(): number {
      return log.filter((l) => l.startsWith('acquire:')).length;
    },
    host: {
      acquire(label: string): number {
        const token = nextToken++;
        live.add(token);
        log.push(`acquire:${label}`);
        return token;
      },
      release(token: number): void {
        live.delete(token);
        log.push(`release:${token}`);
      },
    },
  };
}

// ---------------------------------------------------------------------------
// 每用例：换探针宿主、把控制器指回默认实现；用例后复位（全局可变量不留污染）
// ---------------------------------------------------------------------------
let probe: ReturnType<typeof createProbeHost>;
let HttpServer: any;
let StaticServer: any;
let AppServer: any;
let ConfigServer: any;

beforeEach(async () => {
  probe = createProbeHost();
  ctl = { isRunning: () => Promise.resolve(false), getStats: () => Promise.resolve({}) };
  __setHostHandleRefForTest(probe.host);
  // index.ts 必须动态 import：静态 import 会被提升到模块体之前，
  // 而工厂必须先装好（见文件头「桩的稳定性要求」）。
  ({ HttpServer, StaticServer, AppServer, ConfigServer } = await import('../../src/index'));
});

afterEach(() => {
  __setHostHandleRefForTest(undefined);
  __clearHybridObjectFactory();
});

// ---------------------------------------------------------------------------
// 契约：在途期间持有；settle 后释放
// ---------------------------------------------------------------------------

const SERVER_CLASSES: Array<[string, () => any]> = [
  ['HttpServer', () => new HttpServer()],
  ['StaticServer', () => new StaticServer()],
  ['AppServer', () => new AppServer()],
  ['ConfigServer', () => new ConfigServer()],
];

for (const [name, make] of SERVER_CLASSES) {
  test(`${name}.isRunning() 在途期间持有请求型 ref，settle 后释放`, async () => {
    const { promise, api } = controllableIsRunning();
    ctl.isRunning = () => promise;

    const server = make();
    probe.log.length = 0;

    const pending = server.isRunning();
    // 让 async 体推进到它自己的 await（acquire 若在 await 之前，此刻已发生）
    await Promise.resolve();

    assert.ok(api.called, `${name}.isRunning() 应当调用原生方法`);
    assert.equal(
      probe.held,
      1,
      `${name}.isRunning() 在途期间必须持有 1 个 ref；` +
        `为 0 意味着宿主会在结果回来之前收泵，Promise 永不 settle`
    );

    api.resolve(true);
    assert.equal(await pending, true);

    assert.equal(probe.held, 0, `${name}.isRunning() settle 之后必须释放；泄漏会让进程永不退出`);
    assert.equal(probe.acquires, 1, `${name}.isRunning() 应当恰好 acquire 一次`);
  });
}

test('HttpServer.getStats() 在途期间持有请求型 ref，settle 后释放', async () => {
  let settle: ((v: Record<string, unknown>) => void) | null = null;
  ctl.getStats = () =>
    new Promise<Record<string, unknown>>((resolve) => {
      settle = resolve;
    });

  const server = new HttpServer();
  probe.log.length = 0;

  const pending = server.getStats();
  await Promise.resolve();

  assert.ok(settle !== null, 'getStats() 应当调用原生方法');
  assert.equal(probe.held, 1, 'getStats() 在途期间必须持有 1 个 ref');

  settle!({ ok: true });
  assert.deepEqual(await pending, { ok: true });

  assert.equal(probe.held, 0, 'getStats() settle 之后必须释放');
  assert.equal(probe.acquires, 1, 'getStats() 应当恰好 acquire 一次');
});

// 负对照：失败路径漏 release 就是「每请求泄漏一个 ref」，最终进程永不退出。
// 这条同时排除「断言在失败路径上因 acquire 从未发生而空转通过」。
test('HttpServer.isRunning() 原生失败时也必须释放 ref（try/finally）', async () => {
  const { promise, api } = controllableIsRunning();
  ctl.isRunning = () => promise;

  const server = new HttpServer();
  probe.log.length = 0;

  const pending = server.isRunning();
  await Promise.resolve();
  assert.equal(probe.held, 1, '失败路径同样要在在途期间持有 ref');

  api.reject(new Error('native boom'));
  await assert.rejects(pending, /native boom/);

  assert.equal(probe.held, 0, '失败路径漏 release 会泄漏 ref');
  assert.equal(probe.acquires, 1, '失败路径也应当恰好 acquire 一次（这条排除断言空转）');
});

// 负对照：同步取属性不该登记 ref（否则「不持句柄」的方法会把 loop 永久顶住）。
test('同步 getter 不登记 ref（负对照）', () => {
  const server = new HttpServer();
  probe.log.length = 0;
  void server.port;
  assert.equal(probe.held, 0);
  assert.equal(probe.acquires, 0);
});
