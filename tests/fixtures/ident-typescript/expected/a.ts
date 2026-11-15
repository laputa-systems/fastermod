function renamed(x: number): number { return x + 1; }
interface I { renamed: string }
const v: number = renamed(1);
// helper
const s = `helper ${renamed}`;
