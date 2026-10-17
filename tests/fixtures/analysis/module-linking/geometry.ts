export const PI_ISH = 3.14;

export function perimeter(side: number): number {
  return side * 4;
}

export default function area(side: number): number {
  return side * side * PI_ISH;
}

export const counter = { count: 0 };
