// world: library
// Library mode: exported classes may be constructed and extended by unknown code.

export class Account {
  balance = 0;
  deposit(amount: number): number {
    this.balance += amount /*@declared number*/;
    return this.balance /*@declared number*/;
  }
}

class Internal {
  value = 1;
  get(): number {
    return this.value /*@proven number int*/;
  }
}

export function main(): void {
  const account = new Account();
  console.log(account.deposit(10), new Internal().get());
}
