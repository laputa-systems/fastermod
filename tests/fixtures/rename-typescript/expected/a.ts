interface Config { value: number }

function load(cfg: Config): number {
  return cfg.value;
}

const answer = load({ value: 1 });
console.log(answer);
