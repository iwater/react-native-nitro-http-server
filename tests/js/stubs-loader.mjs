// ESM resolve hook：
//  1) 把 RN 专有的模块名重定向到 tests/js/stubs/ 下的桩；
//  2) 给**无扩展名的相对导入**补扩展名。
//
// 用 resolve hook（而不是改源码或复制逻辑）的好处：测试导入的是
// **src/ 本体**，跑的是真实实现，不存在副本漂移。
//
// 为什么需要 (2)：`src/index.ts` 里是 `import { createServer } from './http'`
// 这种无扩展名导入（tsc 的 classic 风格），而 Node 的 ESM 解析器**不做扩展名补全**，
// 直接报 ERR_MODULE_NOT_FOUND。不补的话 `src/index.ts` 在 Node 里根本 import 不进来，
// 于是 index.ts 里的任何逻辑（含 WebSocket）都无法用桩单测覆盖。

import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));

const STUBS = new Map([
  ['react-native', 'react-native.mjs'],
  ['react-native-nitro-modules', 'react-native-nitro-modules.mjs'],
  ['react-native-nitro-buffer', 'react-native-nitro-buffer.mjs'],
]);

const NOT_FOUND = 'ERR_MODULE_NOT_FOUND';

// 补全顺序：`.ts` 在前（源码），`.js` 次之，最后是目录的 index。
const SUFFIXES = ['.ts', '.js', '.mjs', '/index.ts'];

function isPathLike(specifier) {
  return specifier.startsWith('./') || specifier.startsWith('../') || specifier.startsWith('/');
}

export async function resolve(specifier, context, nextResolve) {
  const stub = STUBS.get(specifier);
  if (stub) {
    return {
      url: pathToFileURL(join(HERE, 'stubs', stub)).href,
      shortCircuit: true,
    };
  }

  if (!isPathLike(specifier)) {
    return nextResolve(specifier, context);
  }

  try {
    return await nextResolve(specifier, context);
  } catch (err) {
    if (err?.code !== NOT_FOUND) throw err;

    // 只在「原样解析不到」时补扩展名：已带扩展名的路径走不到这里。
    for (const suffix of SUFFIXES) {
      try {
        return await nextResolve(specifier + suffix, context);
      } catch (e) {
        if (e?.code !== NOT_FOUND) throw e;
      }
    }
    throw err;
  }
}
