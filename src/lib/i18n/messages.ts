import type { Messages } from "./en";

export type { Messages };

/** 目录里的每个叶子节点，以点分路径表示，例如 `"settings.title"`。 */
type Leaves<T, Prefix extends string = ""> = {
  [K in keyof T & string]: T[K] extends string
    ? `${Prefix}${K}`
    : Leaves<T[K], `${Prefix}${K}.`>;
}[keyof T & string];

export type MessageKey = Leaves<Messages>;

/** 可以填充消息里 `{placeholders}` 的值。 */
export type Vars = Record<string, string | number>;
