import { qux } from './a.js';
import { qux as aliased } from './a.js';
export { qux } from './a.js';
const m = await import('./a.js');
console.log(qux(1), aliased(2), m.qux(3));
