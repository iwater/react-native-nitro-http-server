// 注册桩 loader。用法（在包根目录）：
//   node --import ./tests/js/register.mjs --test tests/js/*.test.ts
import { register } from 'node:module';

register('./stubs-loader.mjs', import.meta.url);
