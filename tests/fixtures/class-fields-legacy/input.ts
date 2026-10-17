// With useDefineForClassFields: false, fields are assigned (not defined) in the constructor
// and fields without initialisers don't exist until assigned.

class Base {
  value = 1;
  set setter(v: number) {
    log.push(`setter ${v}`);
  }
}

class Derived extends Base {
  declare value: number;
  setter = 2;
  uninitialised?: string;
}

const log: string[] = [];

export function main(): void {
  const d = new Derived();
  console.log(log.join(";"), d.value, Object.keys(d).join(","));
  console.log("uninitialised" in d, Object.getOwnPropertyNames(d).length);
}
