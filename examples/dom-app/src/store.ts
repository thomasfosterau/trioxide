import { Filter, Priority, Task } from './task.js';

type Listener<T> = (value: T) => void;

/** A tiny observable value. */
export class Signal<T> {
  #listeners = new Set<Listener<T>>();

  constructor(private current: T) {}

  get value(): T {
    return this.current;
  }

  set(value: T): void {
    if (Object.is(value, this.current)) return;
    this.current = value;
    for (const listener of this.#listeners) listener(value);
  }

  subscribe(listener: Listener<T>): () => void {
    this.#listeners.add(listener);
    listener(this.current);
    return () => this.#listeners.delete(listener);
  }
}

export interface Summary {
  total: number;
  remaining: number;
  byPriority: Record<keyof typeof Priority, number>;
}

export class TaskStore {
  readonly tasks = new Signal<readonly Task[]>([]);
  readonly filter = new Signal<Filter>(Filter.All);

  add(title: string, priority?: Priority): Task | undefined {
    const trimmed = title.trim();
    if (!trimmed) return undefined;
    const task = new Task(trimmed, priority);
    this.tasks.set([...this.tasks.value, task]);
    return task;
  }

  toggle(id: number): void {
    this.tasks.set(this.tasks.value.map((task) => (task.id === id ? task.toggle() : task)));
  }

  remove(id: number): void {
    this.tasks.set(this.tasks.value.filter((task) => task.id !== id));
  }

  visible(): Task[] {
    const filter = this.filter.value;
    return this.tasks.value
      .filter((task) => task.matches(filter))
      .sort((a, b) => b.priority - a.priority || a.id - b.id);
  }

  summary(): Summary {
    const byPriority = { Low: 0, Normal: 0, High: 0 };
    let remaining = 0;
    for (const task of this.tasks.value) {
      byPriority[Priority[task.priority] as keyof typeof Priority]++;
      if (!task.done) remaining++;
    }
    return { total: this.tasks.value.length, remaining, byPriority };
  }
}
