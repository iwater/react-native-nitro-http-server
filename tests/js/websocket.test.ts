// Task 11：TS WebSocket 修复的桩单测
//   ① `ServerWebSocket.send()` 支持 ArrayBufferView
//   ② `ConfigServer.stop()` 关闭并清理连接表
//   ③ autoRestart 后按**重启前的归属**重挂原生 WebSocket 回调
//
// 跑法（包根目录）：npm run test:js
//
// 为什么能测（以及为什么必须动态 import）
// ------------------------------------
// `ServerWebSocket` / `ConfigServer` 都不接受依赖注入，它们直接用模块级的
// `HttpServerModule` —— 而 index.ts 在**模块顶层**就
// `NitroModules.createHybridObject('HttpServer')`。
// 所以：先用 `__setHybridObjectFactory()` 装一个替身，再 `await import()` index.ts。
// ⚠️ ESM 的静态 import 会被提升到模块体之前，静态导入会先炸（桩默认「一用就抛」）。
//
// 三条各自的「有区分力」在哪（修复前必红）
// --------------------------------------
//   ① 断言「传给 wsSendBinary 的第一个数据参数 `instanceof ArrayBuffer`」——
//      修复前传的是 Uint8Array 本身，直接失败。
//      ⚠️ 只断言字节是不够的：`new Uint8Array(view)` 会把视图的元素复制出来，
//      修复前后都等于期望字节，那条断言**没有区分力**。
//   ② 断言 stop() 之后 `getWebSocketConnections().size === 0` + onclose 收到 1001。
//   ③ 用**行为**（哪一方的 handler 收到了事件）而不是函数对象身份来判定 ——
//      身份判定会把「重新创建了一个等价闭包」误判成失败。
//
// ③ 的端到端覆盖为空：无头宿主给 `react-native` 的虚拟模块里 AppState 会**丢掉** handler
// （只有 `addEventListener` 返回一个空壳，不会真的回调），没有触发入口，
// 所以 autoRestart 只能在这一层验证。

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

// 由 tests/js/stubs-loader.mjs 重定向到 tests/js/stubs/
import { __setHybridObjectFactory } from 'react-native-nitro-modules';
import { __emitAppState, __resetAppState } from 'react-native';

// ---------------------------------------------------------------------------
// 原生侧替身
// ---------------------------------------------------------------------------

function createFakeNative() {
  const calls: Array<{ method: string; args: unknown[] }> = [];
  let wsHandler: ((event: unknown) => void) | null = null;
  let nextPort = 18080;

  const ret = (method: string, value: () => unknown) => (...args: unknown[]) => {
    calls.push({ method, args });
    return Promise.resolve(value());
  };
  const void_ = (method: string) => (...args: unknown[]) => {
    calls.push({ method, args });
  };

  const fake = {
    start: ret('start', () => nextPort),
    startServerWithConfig: ret('startServerWithConfig', () => nextPort),
    stop: ret('stop', () => undefined),
    stopAppServer: ret('stopAppServer', () => undefined),
    isRunning: ret('isRunning', () => true),
    getStats: ret('getStats', () => ({})),
    sendBinaryResponse: ret('sendBinaryResponse', () => true),
    wsSendText: ret('wsSendText', () => true),
    wsSendBinary: ret('wsSendBinary', () => true),
    wsClose: ret('wsClose', () => true),
    setCorsConfig: void_('setCorsConfig'),
    setWebSocketHandler: (h: (event: unknown) => void) => {
      calls.push({ method: 'setWebSocketHandler', args: [h] });
      wsHandler = h;
    },
  };

  return {
    fake,
    /** 最近一次某方法调用的参数（没有就抛） */
    last(method: string) {
      const hit = [...calls].reverse().find((c) => c.method === method);
      assert.ok(hit, `原生方法 ${method} 未被调用`);
      return hit.args;
    },
    count(method: string) {
      return calls.filter((c) => c.method === method).length;
    },
    /** 当前装在原生侧的那个 WebSocket 回调 */
    getWsHandler() {
      assert.ok(wsHandler, '原生侧还没有装 WebSocket 回调');
      return wsHandler;
    },
    setNextPort(p: number) {
      nextPort = p;
    },
    reset() {
      calls.length = 0;
      wsHandler = null;
      nextPort = 18080;
    },
  };
}

const harness = createFakeNative();
__setHybridObjectFactory(() => harness.fake);

const mod: typeof import('../../src/index.ts') = await import('../../src/index.ts');

// ---------------------------------------------------------------------------
// 用例之间的隔离
// ---------------------------------------------------------------------------
beforeEach(() => {
  harness.reset();
  __resetAppState();
  // 连接表是模块级的，用例之间会互相污染
  mod.getWebSocketConnections().clear();
});

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** 驱动一次原生 'open' 事件（模拟客户端连上来） */
function openConnection(connectionId: string, path = '/ws') {
  harness.getWsHandler()({
    connectionId,
    type: 'open',
    path,
    query: '',
    headersJson: '{}',
  });
  const ws = mod.getWebSocket(connectionId);
  assert.ok(ws, `连接表里应有 ${connectionId}`);
  return ws;
}

const okHandler = () => ({ statusCode: 200, headers: {}, body: 'ok' });
const wsConfig = { mounts: [{ type: 'websocket', path: '/ws' }] };

/** 触发一次 autoRestart：探测判死 + 重启成功 */
async function triggerAutoRestart(newPort = 18081) {
  const origFetch = globalThis.fetch;
  // _probeAlive 用的是全局 fetch；直接让它失败（确定性，且不依赖端口是否被占）
  globalThis.fetch = (() => Promise.reject(new Error('probe: connection refused'))) as typeof fetch;
  try {
    harness.setNextPort(newPort);
    // 监听器是 async 的，__emitAppState 会把它们 await 完
    await __emitAppState('active');
  } finally {
    globalThis.fetch = origFetch;
  }
}

// ===========================================================================
// ① send() 支持 ArrayBufferView
// ===========================================================================

test('send(): subarray 只发它自己那一段（byteOffset ≠ 0）', async () => {
  const ws = new mod.ServerWebSocket('conn-sub');
  const src = new Uint8Array([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x10, 0x20]);

  await ws.send(src.subarray(2, 6));

  const data = harness.last('wsSendBinary')[1] as ArrayBuffer;
  assert.ok(
    data instanceof ArrayBuffer,
    `必须传真正的 ArrayBuffer，实际拿到 ${Object.prototype.toString.call(data)}` +
      '（原生侧 wsSendBinary 只接受 ArrayBuffer，视图会被 Nitro 转换器拒绝）'
  );
  assert.deepEqual([...new Uint8Array(data)], [0xcc, 0xdd, 0xee, 0xff]);
});

test('send(): 整段覆盖的视图直接复用底层 buffer（不额外拷贝）', async () => {
  const ws = new mod.ServerWebSocket('conn-full');
  const backing = new ArrayBuffer(4);

  await ws.send(new Uint8Array(backing));

  assert.equal(harness.last('wsSendBinary')[1], backing, 'byteOffset=0 且长度相等时应直传 .buffer');
});

test('send(): ArrayBuffer 直传（同一对象，不拷贝）', async () => {
  const ws = new mod.ServerWebSocket('conn-ab');
  const ab = new ArrayBuffer(3);

  await ws.send(ab);

  assert.equal(harness.last('wsSendBinary')[1], ab);
});

test('send(): DataView 按 byteOffset/byteLength 切片', async () => {
  const ws = new mod.ServerWebSocket('conn-dv');
  const backing = new Uint8Array([1, 2, 3, 4, 5, 6]).buffer;

  await ws.send(new DataView(backing, 1, 3));

  const data = harness.last('wsSendBinary')[1] as ArrayBuffer;
  assert.ok(data instanceof ArrayBuffer, 'DataView 也必须归一化成 ArrayBuffer');
  assert.deepEqual([...new Uint8Array(data)], [2, 3, 4]);
});

test('send(): Node Buffer（Uint8Array 子类，带 byteOffset）只发自己的字节', async () => {
  const ws = new mod.ServerWebSocket('conn-buf');
  // nitro-buffer 的 Buffer 与 Node 的一样是 Uint8Array 子类；这里用显式 byteOffset
  // 构造，避免依赖 Node 的 pool 行为（pool 下 offset 是偶然值）
  const b = Buffer.from(new Uint8Array([0x41, 0x42, 0x43, 0x44, 0x45]).buffer, 1, 3);

  await ws.send(b);

  const data = harness.last('wsSendBinary')[1] as ArrayBuffer;
  assert.ok(data instanceof ArrayBuffer, 'Buffer 也必须归一化成 ArrayBuffer');
  assert.deepEqual([...new Uint8Array(data)], [0x42, 0x43, 0x44]);
});

test('send(): 字符串仍走文本通道', async () => {
  const ws = new mod.ServerWebSocket('conn-text');

  await ws.send('hello');

  assert.deepEqual(harness.last('wsSendText'), ['conn-text', 'hello']);
  assert.equal(harness.count('wsSendBinary'), 0);
});

test('send(): 非 OPEN 状态抛错，且不产生原生调用', async () => {
  const ws = new mod.ServerWebSocket('conn-closed');
  ws._handleClose(1000, 'bye');

  await assert.rejects(() => ws.send('x'), /not open/);
  assert.equal(harness.count('wsSendText'), 0);
  assert.equal(harness.count('wsSendBinary'), 0);
});

// ===========================================================================
// ② stop() 清理连接表
// ===========================================================================

test('stop(): 关闭并清理所有连接，onclose 收到 1001，readyState 不再停在 OPEN', async () => {
  const server = new mod.ConfigServer();
  const closed: Array<{ code: number; reason: string; wasClean: boolean }> = [];
  server.onWebSocket('/ws', (ws) => {
    ws.onclose = (e) => closed.push(e);
  });
  const port = await server.start(0, okHandler, wsConfig, { host: '127.0.0.1' });
  assert.ok(port > 0);

  const a = openConnection('c1');
  const b = openConnection('c2');
  assert.equal(mod.getWebSocketConnections().size, 2);

  await server.stop();

  assert.equal(
    mod.getWebSocketConnections().size,
    0,
    'stop() 之后连接表必须为空 —— 否则 getWebSocketConnections() 会一直返回死连接'
  );
  assert.equal(a.readyState, 3, 'readyState 必须变成 CLOSED，不能停在 OPEN');
  assert.equal(b.readyState, 3);
  assert.equal(closed.length, 2, '两个连接都应触发 onclose');
  assert.equal(closed[0].code, 1001, '1001 = Going Away');
  assert.equal(closed[0].reason, 'server stopped');
  assert.equal(closed[0].wasClean, false, '1001 不是 clean close');
  assert.deepEqual(harness.last('wsClose'), ['c2', 1001, 'server stopped']);
});

test('stop(): 未运行时提前返回，但仍不应留下残留连接', async () => {
  const server = new mod.ConfigServer();
  // 从未 start()：连接表里若有残留（例如上一个实例留下的），stop() 不该假装没事
  const ws = new mod.ServerWebSocket('orphan');
  mod.getWebSocketConnections().set('orphan', ws);

  await server.stop();

  assert.equal(
    mod.getWebSocketConnections().size,
    0,
    'stop() 是「这个包不再持有连接」的语义，未运行也要清'
  );
});

// ===========================================================================
// ③ autoRestart 后重挂回调（按重启前的归属）
// ===========================================================================

test('autoRestart: 独立 handler 最后安装 → 重启后仍是它生效', async () => {
  const seen = { standalone: 0, config: 0 };

  const server = new mod.ConfigServer();
  server.onWebSocket('/ws', () => { seen.config++; });
  await server.start(0, okHandler, wsConfig, { host: '127.0.0.1', autoRestart: true });

  // 独立入口在 ConfigServer 之后安装 → 它生效（原生侧后装者胜）
  mod.setupWebSocketHandler(() => { seen.standalone++; });

  openConnection('pre');
  assert.deepEqual(seen, { standalone: 1, config: 0 }, '重启前：独立 handler 生效');

  await triggerAutoRestart();

  openConnection('post');
  assert.deepEqual(
    seen,
    { standalone: 2, config: 0 },
    '重启后独立 handler 必须仍然生效（否则用户注册的 WebSocket 处理器在重启后静默丢失）'
  );

  await server.stop();
});

test('autoRestart: ConfigServer 最后安装 → 重启后仍是它生效（不能被独立 handler 抢走）', async () => {
  const seen = { standalone: 0, config: 0 };

  const server = new mod.ConfigServer();
  // 独立入口在 ConfigServer.start() 之前安装 → start() 覆盖它，ConfigServer 生效
  mod.setupWebSocketHandler(() => { seen.standalone++; });
  server.onWebSocket('/ws', () => { seen.config++; });
  await server.start(0, okHandler, wsConfig, { host: '127.0.0.1', autoRestart: true });

  openConnection('pre');
  assert.deepEqual(seen, { standalone: 0, config: 1 }, '重启前：ConfigServer 的 handler 生效');

  await triggerAutoRestart();

  openConnection('post');
  assert.deepEqual(
    seen,
    { standalone: 0, config: 2 },
    '重启后必须保持同一归属 —— 无条件重挂独立 handler 会把 ConfigServer 的处理器挤掉'
  );

  await server.stop();
});

test('autoRestart: 重启会清理旧连接（旧服务已停，连接全部失效）', async () => {
  const server = new mod.ConfigServer();
  server.onWebSocket('/ws', () => {});
  await server.start(0, okHandler, wsConfig, { host: '127.0.0.1', autoRestart: true });

  openConnection('stale');
  assert.equal(mod.getWebSocketConnections().size, 1);

  await triggerAutoRestart();

  assert.equal(
    mod.getWebSocketConnections().size,
    0,
    '每次自动重启都会让全部连接失效，不清理就是每次漏一批'
  );

  await server.stop();
});

test('autoRestart: 重启失败时不改动回调归属', async () => {
  const seen = { standalone: 0, config: 0 };
  const server = new mod.ConfigServer();
  server.onWebSocket('/ws', () => { seen.config++; });
  await server.start(0, okHandler, wsConfig, { host: '127.0.0.1', autoRestart: true });
  mod.setupWebSocketHandler(() => { seen.standalone++; });

  const before = harness.count('setWebSocketHandler');
  harness.setNextPort(0); // 重启返回 0 = 失败
  const origFetch = globalThis.fetch;
  globalThis.fetch = (() => Promise.reject(new Error('probe'))) as typeof fetch;
  try {
    await __emitAppState('active');
  } finally {
    globalThis.fetch = origFetch;
  }
  await sleep(20);

  assert.equal(
    harness.count('setWebSocketHandler'),
    before,
    '重启没成功就不该重挂回调（此时服务是停的，装了也没用，还会掩盖失败）'
  );

  await server.stop();
});
