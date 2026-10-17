import { Fragment, h, render } from "./runtime.js";

interface ItemProps {
  label: string;
  done?: boolean;
}

function Item({ label, done = false }: ItemProps) {
  return <li class={done ? "done" : "todo"}>{label}</li>;
}

function List(props: { items: ItemProps[]; title: string }) {
  return (
    <>
      <h2>{props.title}</h2>
      <ul>
        {props.items.map((item) => (
          <Item {...item} />
        ))}
      </ul>
      {props.items.length === 0 && <p>empty</p>}
    </>
  );
}

export function main(): void {
  const items: ItemProps[] = [{ label: "write plan", done: true }, { label: "build trioxide" }];
  console.log(render(<List title="Todo" items={items} />));
  console.log(render(<List title="None" items={[]} />));
}
