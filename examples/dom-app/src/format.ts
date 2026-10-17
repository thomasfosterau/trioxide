import type { Summary } from './store.js';

export function plural(count: number, noun: string, suffix = 's'): string {
  return `${count} ${noun}${count === 1 ? '' : suffix}`;
}

export function describe({ total, remaining, byPriority }: Summary): string {
  if (total === 0) return 'Nothing to do.';
  const parts = Object.entries(byPriority)
    .filter(([, count]) => count > 0)
    .map(([priority, count]) => `${count} ${priority.toLowerCase()}`);
  return `${plural(remaining, 'task')} left of ${total} (${parts.join(', ')}).`;
}
