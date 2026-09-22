// Node 测试用的 `react-native` 桩。
//
// 存在的理由：本包没有 JS 测试基础设施，而 TS 层的顺序保证（Task 10 的
// `_writeChain`）只有在真实 import 图下才测得到 —— 复制一份逻辑出来测会漂移。
// 这里只提供被 import 的符号，以及**驱动 autoRestart 所需的最小可观测性**。
//
// ⚠️ 必须把 **类型** 也导出一份：Node 的类型剥离只认 `import type`，
// `import { AppState, AppStateStatus } from 'react-native'` 里的
// `AppStateStatus` 在运行时仍会被当作真导入解析。

/** 已注册的 AppState 监听器。`ConfigServer._registerAutoRestart` 把回调挂在这里。 */
const appStateListeners = new Set();

export const AppState = {
  currentState: 'active',
  addEventListener: (type, callback) => {
    if (type === 'change') appStateListeners.add(callback);
    return {
      remove() {
        appStateListeners.delete(callback);
      },
    };
  },
};

/**
 * 测试用：模拟 App 状态变化，同步把所有 'change' 监听器都调一遍。
 *
 * 注意 `_registerAutoRestart` 的回调是 **async** 的（内部 await `_probeAlive()`），
 * 所以这里返回的 Promise 只能保证「回调被启动了」，不保证内部已经跑完 ——
 * 调用方需要自己 await 到可观测的副作用（例如原生调用记录）。
 */
export function __emitAppState(state) {
  const pending = [];
  for (const cb of appStateListeners) {
    pending.push(cb(state));
  }
  return Promise.all(pending);
}

/** 测试用：清掉所有 AppState 监听器（用例之间隔离）。 */
export function __resetAppState() {
  appStateListeners.clear();
}

// 仅类型，运行时不会被用到；导出是为了让 ESM 解析不报 "does not provide an export"
export const AppStateStatus = undefined;
