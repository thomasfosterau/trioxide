export enum Priority {
  Low = 1,
  Normal,
  High,
}

export const enum Filter {
  All = 'all',
  Active = 'active',
  Done = 'done',
}

export interface TaskData {
  readonly id: number;
  title: string;
  priority: Priority;
  done: boolean;
}

export class Task implements TaskData {
  static #nextId = 1;
  readonly id = Task.#nextId++;
  done = false;

  constructor(
    public title: string,
    public priority: Priority = Priority.Normal,
  ) {}

  toggle(): this {
    this.done = !this.done;
    return this;
  }

  matches(filter: Filter): boolean {
    switch (filter) {
      case Filter.Active:
        return !this.done;
      case Filter.Done:
        return this.done;
      default:
        return true;
    }
  }
}

export function parsePriority(value: string | null | undefined): Priority {
  switch (value?.toLowerCase()) {
    case 'low':
      return Priority.Low;
    case 'high':
      return Priority.High;
    default:
      return Priority.Normal;
  }
}
