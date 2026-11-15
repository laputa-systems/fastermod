import { helper } from './mod.js';

let total = 1;

export function bump() {
  total += 1;
  return total;
}

const obj = { counter: total };
export { total as counter };
console.log(helper(total));
