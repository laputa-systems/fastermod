export function qux(x) { return x + 1; }
const local = qux(2);
const obj = { foo: qux };
export { qux as renamed };
