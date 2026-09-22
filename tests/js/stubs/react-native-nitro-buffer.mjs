// Node 测试用的 `react-native-nitro-buffer` 桩。
//
// Node 自带 Buffer，语义与 RN 版一致（`Buffer.isBuffer` / `toString(encoding)`
// 都是本包在 `http.ts` 里实际用到的部分），直接转发即可。

export const Buffer = globalThis.Buffer;
export default globalThis.Buffer;
