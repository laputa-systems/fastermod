function helper(x: number): number { return x + 1; }
interface I { helper: string }
const v: number = helper(1);
// helper
const s = `helper ${helper}`;
