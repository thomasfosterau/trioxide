// Conformance driver (not part of mitt): a deterministic tour of the API whose output the
// harness compares across builds.
import mitt, { type Emitter } from "./src/index.js";

type Events = { add: number; label: string; clear: undefined };

export function main(): void {
  const log: string[] = [];
  const events: Emitter<Events> = mitt<Events>();
  let total = 0;
  const add = (n: number) => {
    total += n;
    log.push(`add ${n} -> ${total}`);
  };
  events.on("add", add);
  events.on("label", (s) => log.push(`label ${s.toUpperCase()}`));
  events.on("*", (type, event) => log.push(`* ${String(type)} ${JSON.stringify(event)}`));
  for (let i = 1; i <= 5; i++) events.emit("add", i * i);
  events.emit("label", "trioxide");
  events.emit("clear");
  events.off("add", add);
  events.emit("add", 100);
  events.off("label");
  events.emit("label", "gone");
  // Handlers added while emitting don't run for that emit.
  events.on("add", function again(n) {
    log.push(`again ${n}`);
    events.on("add", (m) => log.push(`late ${m}`));
  });
  events.emit("add", 1);
  events.emit("add", 2);
  log.push(`handlers: ${[...events.all.keys()].map(String).join(",")}`);
  events.all.clear();
  events.emit("add", 3);
  console.log(log.join("\n"));
  console.log(`total ${total}`);
}
