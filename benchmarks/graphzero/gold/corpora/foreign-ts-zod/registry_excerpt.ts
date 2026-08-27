// Foreign TypeScript fixture: zod-style registry with interface dispatch,
// a class implementation, and a re-exported helper module.
import { normalizeKey } from "./util.js";
import type { $ZodType } from "./schemas.js";
export { $ZodCheck } from "./checks.js";

interface Registry {
  add(key: string, value: string): void;
  get(key: string): string | undefined;
}

class MapRegistry implements Registry {
  private entries = new Map<string, string>();

  add(key: string, value: string): void {
    this.entries.set(normalizeKey(key), value);
  }

  get(key: string): string | undefined {
    return this.entries.get(normalizeKey(key));
  }
}

function register(registry: Registry, key: string, schema: $ZodType): void {
  registry.add(key, String(schema));
}

function unusedHelper(key: string): string {
  return key.toUpperCase();
}
