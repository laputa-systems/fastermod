export function foo(x) { return x + 1; }
const local = foo(2);
const obj = { foo };
export { foo as renamed };
