// Foreign TypeScript fixture: zod v4 declaration-style imports and type aliases.
import * as checks from "./checks.js";
import type * as errors from "./errors.js";
import * as schemas from "./schemas.js";
import * as util from "./util.js";

export type Params<T extends schemas.$ZodType | checks.$ZodCheck, IssueTypes extends errors.$ZodIssueBase> =
  util.Flatten<Partial<T & { error?: string | errors.$ZodErrorMap<IssueTypes> | undefined }>>;

interface Parser<T> {
  parse(input: unknown): T;
}

function parseWith<T>(parser: Parser<T>, input: unknown): T {
  return parser.parse(input);
}
