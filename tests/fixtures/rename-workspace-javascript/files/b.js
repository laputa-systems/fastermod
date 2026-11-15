import { foo } from './a.js';
import { foo as aliased } from './a.js';
export { foo } from './a.js';
const m = await import('./a.js');
console.log(foo(1), aliased(2), m.foo(3));
