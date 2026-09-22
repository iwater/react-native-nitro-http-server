/**
 * LoopRef —— Node handle-ref 模型到「宿主能力」的桥。
 *
 * 背景：Node 里 listening server / connected socket 本身就顶住事件循环
 * （libuv 的 handle ref）。本库的宿主未必有这个语义：
 *   - 无头 JSI 宿主：EventLoop 在「无待办」时收泵，需要显式保活；
 *     这类宿主提供 globalThis.__qjsHandleRef 供库登记 handle 存活；
 *   - React Native：App 生命周期归 OS，JS 空转不退出 —— 没有这个全局，
 *     本类所有方法是 no-op，行为与今天完全一致。
 *
 * 每 handle 一个 LoopRef 实例；acquire/release 幂等（重复调用不双计/不双减）。
 *
 * 与 react-native-nitro-net / -dns / -udp 的同名文件**代码逐字相同，仅本段头注释
 * 各持一份**（约定各库各持一份、独立演进，不抽共享包；加这段说明本身就意味着不再是
 * 逐字相同）。本库只用到 handle 型：running 的 server 顶住
 * loop，stop() 成功时释放。
 */

interface HostHandleRef {
  acquire(label: string): number;
  release(token: number): void;
}

// 模块加载时探测一次。宿主能力不会在进程中途出现/消失。
let host: HostHandleRef | undefined =
  (globalThis as { __qjsHandleRef?: HostHandleRef }).__qjsHandleRef;

/** 仅测试用：替换/清除探测到的宿主能力。必须在创建 LoopRef 实例之前调用 —— 存活实例持有的 token 仍由上一个宿主签发。 */
export function __setHostHandleRefForTest(h: HostHandleRef | undefined): void {
  host = h;
}

export class LoopRef {
  private token: number | null = null;
  private readonly label: string;

  constructor(label: string) {
    this.label = label;
  }

  /** 已持有时 no-op。 */
  acquire(): void {
    if (host && this.token === null) {
      this.token = host.acquire(this.label);
    }
  }

  /** 未持有时 no-op。 */
  release(): void {
    if (host && this.token !== null) {
      const t = this.token;
      this.token = null;
      host.release(t);
    }
  }

  /** 当前是否正顶着 loop（诊断/测试用）。 */
  get refed(): boolean {
    return this.token !== null;
  }
}
