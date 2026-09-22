// Task 10：ServerResponse 的写入顺序保证
//
// 运行（在包根目录）：
//   node --import ./tests/js/register.mjs --test tests/js/http_write_order.test.ts
//
// 为什么能测：`ServerResponse` 的构造函数是依赖注入的
// （`constructor(requestId, nativeServer, resolveNativeRequest)`），
// 所以可以注入一个记录调用顺序的桩，不需要原生运行时。
// RN 专有的 import（react-native / nitro-modules / nitro-buffer）
// 由 `register.mjs` 的 resolve hook 重定向到 tests/js/stubs/。
//
// ⚠️ 桩的耗时设计是这条测试**有区分力**的关键：让**先调用的更慢**
// （a=30ms, b=20ms, c=10ms）。如果实现没有串行化，回调会以相反顺序落地，
// 断言就会红。若所有桩都瞬时完成，并发实现也「碰巧」是顺序的 —— 测不出东西。

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { ServerResponse } from '../../src/http.ts';

const DELAYS: Record<string, number> = { a: 30, b: 20, c: 10 };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface StubServer {
  writeResponseChunk(requestId: string, chunk: string): Promise<boolean>;
  endResponse(requestId: string, statusCode: number, headersJson: string): Promise<boolean>;
  sendBinaryResponse(
    requestId: string,
    statusCode: number,
    headersJson: string,
    body: ArrayBuffer
  ): Promise<boolean>;
}

function makeHarness() {
  const log: string[] = [];

  const server: StubServer = {
    async writeResponseChunk(_rid, chunk) {
      await sleep(DELAYS[chunk] ?? 0);
      log.push(`write:${chunk}`);
      return true;
    },
    async endResponse() {
      log.push('end');
      return true;
    },
    async sendBinaryResponse() {
      log.push('binary');
      return true;
    },
  };

  const res = new ServerResponse('req-1', server as never, () => {});
  return { res, log };
}

/** 等到 log 长度达到预期（或超时）。串行链是异步落地的，断言前要等。 */
async function waitForLength(log: string[], n: number, timeoutMs = 3000) {
  const start = Date.now();
  while (log.length < n) {
    if (Date.now() - start > timeoutMs) {
      throw new Error(`等待 log 长度到 ${n} 超时，当前: ${JSON.stringify(log)}`);
    }
    await sleep(5);
  }
}

test('连续 write 必须按调用顺序到达原生层', async () => {
  const { res, log } = makeHarness();

  res.write('a');
  res.write('b');
  res.write('c');
  res.end();

  await waitForLength(log, 4);
  assert.deepEqual(log, ['write:a', 'write:b', 'write:c', 'end']);
});

test('write + end(chunk) 的顺序：chunk 必须在 endResponse 之前', async () => {
  const { res, log } = makeHarness();

  res.write('a');
  res.write('b');
  res.end('c');

  await waitForLength(log, 4);
  assert.deepEqual(log, ['write:a', 'write:b', 'write:c', 'end']);
});

test('write + end(ArrayBuffer)：二进制响应也必须排在挂起的 write 之后', async () => {
  const { res, log } = makeHarness();

  res.write('a');
  res.end(new ArrayBuffer(4));

  await waitForLength(log, 2);
  assert.deepEqual(
    log,
    ['write:a', 'binary'],
    '二进制路径若不走写入链，sendBinaryResponse 会抢在 write 前面 —— ' +
      '而迟到的 writeResponseChunk 会在 cleanup 之后重建累积器条目（永久泄漏）'
  );
});

test('没有 chunk 的 end() 也要排在挂起的 write 之后', async () => {
  const { res, log } = makeHarness();

  res.write('a');
  res.end();

  await waitForLength(log, 2);
  assert.deepEqual(log, ['write:a', 'end']);
});

test('end() 之后 write 应被拒绝，且不产生原生调用', async () => {
  const { res, log } = makeHarness();

  res.end();
  await waitForLength(log, 1);

  const errors: unknown[] = [];
  res.on('error', (e: unknown) => errors.push(e));

  assert.equal(res.write('late'), false);
  assert.equal(errors.length, 1, '应 emit 一次 error');

  await sleep(50);
  assert.deepEqual(log, ['end'], 'end 之后的 write 不应产生原生调用');
});

test('某次 write 失败不应打断后续顺序（error 之后仍按序执行）', async () => {
  const log: string[] = [];
  const server: StubServer = {
    async writeResponseChunk(_rid, chunk) {
      if (chunk === 'a') throw new Error('boom');
      await sleep(DELAYS[chunk] ?? 0);
      log.push(`write:${chunk}`);
      return true;
    },
    async endResponse() {
      log.push('end');
      return true;
    },
    async sendBinaryResponse() {
      log.push('binary');
      return true;
    },
  };

  const res = new ServerResponse('req-2', server as never, () => {});
  const errors: unknown[] = [];
  res.on('error', (e: unknown) => errors.push(e));

  res.write('a');
  res.write('b');
  res.end();

  await waitForLength(log, 2);
  assert.equal(errors.length, 1, '失败的 write 应 emit 一次 error');
  assert.deepEqual(log, ['write:b', 'end'], 'a 失败后 b 与 end 仍应按序执行');
});
