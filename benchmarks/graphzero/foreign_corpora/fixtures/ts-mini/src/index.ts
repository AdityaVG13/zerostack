// Foreign mini TypeScript corpus for GraphZero rebaseline (not GraphZero source).
export type Config = { name: string; enabled: boolean };

export function parseConfig(input: string): Config {
  return { name: input.trim(), enabled: true };
}

export function runIndex(cfg: Config): number {
  return cfg.enabled ? cfg.name.length : 0;
}
