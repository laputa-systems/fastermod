import { helper } from './mod.js';

let counter = 1;

export function bump() {
  counter += 1;
  return counter;
}

const obj = { counter };
export { counter };
console.log(helper(counter));
