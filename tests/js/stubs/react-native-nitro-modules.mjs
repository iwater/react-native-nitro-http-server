// Node 测试用的 `react-native-nitro-modules` 桩。
//
// `NitroModules.createHybridObject` 需要真实原生运行时，在 Node 里必然不可用。
// 两种用法：
//
//  1. **依赖注入（`http_write_order.test.ts` 用的就是这条）**：`ServerResponse` 的
//     构造函数是依赖注入的（`constructor(requestId, nativeServer, resolveNativeRequest)`），
//     直接传桩对象即可，根本不经过 createHybridObject。
//  2. **装工厂（碰 `src/index.ts` 的用例必须用这条）**：index.ts 在**模块顶层**就
//     `createHybridObject('HttpServer')`，所以用例必须先 `__setHybridObjectFactory()`
//     装一个替身。⚠️ ESM 的静态 import 会被提升到模块体之前 —— 要用
//     `await import()` 动态导入 index.ts。
//
// 没有装工厂时让它**抛错而不是静默返回 undefined**：万一有人不小心构造了
// `HttpServer`（它会调 createHybridObject），应当立刻看到明确的失败原因。

let factory = null;

/** 测试用：装一个 createHybridObject 的替身工厂。必须在 import src/index.ts 之前调用。 */
export function __setHybridObjectFactory(fn) {
  factory = fn;
}

/** 测试用：卸掉替身工厂，恢复「一用就抛」的行为。 */
export function __clearHybridObjectFactory() {
  factory = null;
}

export const NitroModules = {
  createHybridObject: (name) => {
    if (factory) return factory(name);
    throw new Error(
      `NitroModules.createHybridObject('${name}') 需要原生运行时，Node 测试里不可用。` +
        `请直接构造 ServerResponse 并注入桩 nativeServer，` +
        `或先调用 __setHybridObjectFactory() 装一个替身。`
    );
  },
};

export default { NitroModules };
