const calls: string[] = [];

function logged(target: object, key: string, descriptor: PropertyDescriptor): PropertyDescriptor {
  const original = descriptor.value as (...args: unknown[]) => unknown;
  descriptor.value = function (this: unknown, ...args: unknown[]) {
    calls.push(`${key}(${args.join(", ")})`);
    return original.apply(this, args);
  };
  return descriptor;
}

function sealed(constructor: Function): void {
  Object.seal(constructor);
  Object.seal(constructor.prototype);
}

@sealed
class Calculator {
  @logged
  add(a: number, b: number): number {
    return a + b;
  }

  @logged
  scale(value: number, factor = 2): number {
    return value * factor;
  }
}

export function main(): void {
  const calc = new Calculator();
  console.log(calc.add(1, 2), calc.scale(5));
  console.log(calls.join(" "));
  console.log(Object.isSealed(Calculator), Object.isSealed(Calculator.prototype));
}
