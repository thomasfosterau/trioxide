// A parent constructor calls an overridden method before the subclass's fields exist.
class Parent {
  constructor() {
    this.show();
  }
  show(): void {}
}
class Child extends Parent {
  y = 5;
  show(): void {
    const v = this.y;
    console.log(v);
  }
}

// A method called from the constructor before an assignment.
class Early {
  x: number;
  constructor() {
    this.peek();
    this.x = 1;
  }
  peek(): void {
    const v = this.x;
    console.log(v);
  }
}

// A static getter.
class Statics {
  static get label(): string {
    return "s";
  }
}

export function main(): void {
  new Child();
  new Early();
  const l = Statics.label;
  console.log(l);
}
