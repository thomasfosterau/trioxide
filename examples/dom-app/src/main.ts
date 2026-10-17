import { describe } from './format.js';
import { TaskStore } from './store.js';
import { Filter, parsePriority, type Task } from './task.js';

function $<T extends HTMLElement>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`missing ${selector}`);
  return element;
}

const store = new TaskStore();
const form = $<HTMLFormElement>('#new-task');
const list = $<HTMLUListElement>('#tasks');
const filters = $<HTMLElement>('#filters');
const summary = $<HTMLParagraphElement>('#summary');

const FILTERS: ReadonlyArray<[Filter, string]> = [
  [Filter.All, 'All'],
  [Filter.Active, 'Active'],
  [Filter.Done, 'Done'],
];

function renderTask(task: Task): HTMLLIElement {
  const item = document.createElement('li');
  item.dataset.id = String(task.id);
  item.dataset.priority = parsePriorityName(task.priority);
  item.classList.toggle('done', task.done);
  const checkbox = Object.assign(document.createElement('input'), { type: 'checkbox', checked: task.done });
  checkbox.addEventListener('change', () => store.toggle(task.id));
  const label = Object.assign(document.createElement('span'), { textContent: task.title });
  const remove = Object.assign(document.createElement('button'), { textContent: '×', title: 'Remove' });
  remove.addEventListener('click', () => store.remove(task.id));
  item.append(checkbox, label, remove);
  return item;
}

function parsePriorityName(priority: number): string {
  return ['', 'low', 'normal', 'high'][priority] ?? 'normal';
}

function render(): void {
  list.replaceChildren(...store.visible().map(renderTask));
  summary.textContent = describe(store.summary());
  for (const button of filters.querySelectorAll<HTMLButtonElement>('button')) {
    button.setAttribute('aria-pressed', String(button.value === store.filter.value));
  }
}

filters.append(
  ...FILTERS.map(([filter, text]) => {
    const button = Object.assign(document.createElement('button'), { type: 'button', value: filter, textContent: text });
    button.addEventListener('click', () => store.filter.set(filter));
    return button;
  }),
);

form.addEventListener('submit', (event) => {
  event.preventDefault();
  const data = new FormData(form);
  const task = store.add(String(data.get('title') ?? ''), parsePriority(data.get('priority') as string | null));
  if (task) console.log(`added #${task.id}: ${task.title}`);
  form.reset();
});

store.tasks.subscribe(render);
store.filter.subscribe(render);
