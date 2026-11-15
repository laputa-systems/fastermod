interface Config { value: number }

function load(cfg: Config): number {
  return cfg.value;
}

const result = load({ value: 1 });
console.log(result);
