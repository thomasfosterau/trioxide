// Application-style code: events, generics, records, closures and string handling.

type Listener<T> = (event: T) => void;

export class Emitter<Events extends Record<string, unknown>> {
  private listeners: { [K in keyof Events]?: Listener<Events[K]>[] } = {};

  on<K extends keyof Events>(name: K, listener: Listener<Events[K]>): () => void {
    const list = (this.listeners[name] ??= []);
    list.push(listener);
    return () => {
      const index = list.indexOf(listener);
      if (index >= 0) list.splice(index, 1);
    };
  }

  emit<K extends keyof Events>(name: K, event: Events[K]): void {
    for (const listener of this.listeners[name] ?? []) listener(event);
  }
}

interface Todo {
  id: number;
  title: string;
  done: boolean;
  tags: string[];
}

export class Store {
  private todos = new Map<number, Todo>();
  private nextId = 1;
  readonly events = new Emitter<{ added: Todo; removed: number }>();

  add(title: string, ...tags: string[]): Todo {
    const todo: Todo = { id: this.nextId++, title: title.trim(), done: false, tags };
    this.todos.set(todo.id, todo);
    this.events.emit("added", todo);
    return todo;
  }

  toggle(id: number): boolean {
    const todo = this.todos.get(id);
    if (!todo) return false;
    todo.done = !todo.done;
    return true;
  }

  remove(id: number): void {
    if (this.todos.delete(id)) this.events.emit("removed", id);
  }

  query({ done, tag }: { done?: boolean; tag?: string } = {}): Todo[] {
    return [...this.todos.values()]
      .filter((todo) => done === undefined || todo.done === done)
      .filter((todo) => !tag || todo.tags.includes(tag))
      .sort((a, b) => a.title.localeCompare(b.title));
  }

  summary(): string {
    const all = this.query();
    const done = all.filter((t) => t.done).length;
    return `${done}/${all.length} done` + (all.length ? `, next: ${all.find((t) => !t.done)?.title ?? "-"}` : "");
  }

  toJSON(): Record<string, Todo> {
    return Object.fromEntries([...this.todos].map(([id, todo]) => [String(id), todo]));
  }
}

export function memoize<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => R {
  const cache = new Map<string, R>();
  return (...args) => {
    const key = JSON.stringify(args);
    let value = cache.get(key);
    if (value === undefined) {
      value = fn(...args);
      cache.set(key, value);
    }
    return value;
  };
}

export const slow = memoize((n: number): number => (n < 2 ? n : slow(n - 1) + slow(n - 2)));

export function parseQuery(query: string): Record<string, string[]> {
  const result: Record<string, string[]> = {};
  for (const part of query.replace(/^\?/, "").split("&")) {
    if (!part) continue;
    const [rawKey, rawValue = ""] = part.split("=");
    const key = decodeURIComponent(rawKey);
    (result[key] ||= []).push(decodeURIComponent(rawValue.replace(/\+/g, " ")));
  }
  return result;
}

export function debounce<T extends (...args: never[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return ((...args: Parameters<T>) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  }) as T;
}

export class Result<T, E = Error> {
  private constructor(private readonly value: T | undefined, private readonly error: E | undefined) {}
  static ok<T>(value: T): Result<T, never> {
    return new Result<T, never>(value, undefined);
  }
  static err<E>(error: E): Result<never, E> {
    return new Result<never, E>(undefined, error);
  }
  map<U>(fn: (value: T) => U): Result<U, E> {
    return this.error === undefined ? Result.ok(fn(this.value as T)) : Result.err(this.error);
  }
  unwrapOr(fallback: T): T {
    return this.error === undefined ? (this.value as T) : fallback;
  }
}

export const store = new Store();
store.add("write corpus", "test");
store.add("match spans", "test", "tsrs");
store.toggle(1);
export const report = store.summary() + JSON.stringify(store, null, 2).length;
export const parsed = Result.ok(parseQuery("?a=1&b=2&a=3")).map((q) => Object.keys(q).length).unwrapOr(0);
