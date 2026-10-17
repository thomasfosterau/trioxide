export interface Named {
  name: string;
}

export const label = (value: Named): string => `<${value.name}>`;
