import type { Messages } from "./en";

export type { Messages };

/** Every leaf of the catalogue as a dotted path, e.g. `"settings.title"`. */
type Leaves<T, Prefix extends string = ""> = {
  [K in keyof T & string]: T[K] extends string
    ? `${Prefix}${K}`
    : Leaves<T[K], `${Prefix}${K}.`>;
}[keyof T & string];

export type MessageKey = Leaves<Messages>;

/** Values a message's `{placeholders}` can be filled with. */
export type Vars = Record<string, string | number>;
