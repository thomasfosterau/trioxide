// Strings with lone surrogates or invisible characters stay where they are written.
const lone = "\uD83D";
const bom = "﻿";
const joiner = "a‍b";
const template = `\uDE00`;
export function main(): void {
  console.log(lone.length, (lone + "\uDE00").codePointAt(0), bom.length, joiner.length);
  console.log(template.length, ("\uD83D" + "\uDE00").codePointAt(0));
}
