// JSON.parse produces values trioxide knows nothing about; JSON.stringify reads its argument.

interface Settings {
  width: number;
}

export function main(): void {
  const settings: Settings = JSON.parse('{"width": "wide"}');
  // TypeScript believes the annotation; the value is a string.
  console.log(settings.width /*@declared number*/, typeof settings.width);
  const text = JSON.stringify({ a: 1 });
  console.log(text /*@declared string*/);
}
