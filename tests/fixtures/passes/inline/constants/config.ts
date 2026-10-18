// Constants another module imports. The enum comes first: code before it must not be able
// to call anything, or a read could run before the enum object exists.
export enum Color {
  Red = 1,
  Green = 2,
  Blue = Red | Green,
}

export const LIMIT = 8;
export const NAME = "trioxide";
export const ENABLED = true;
export const NOTHING = null;
export const HUGE = 10n ** 20n;
export const ONE_BIG = 12345678901234567890n;
export const OFFSET = -3;
export const BANNER = "a string long enough that copying it into every use would hurt";
