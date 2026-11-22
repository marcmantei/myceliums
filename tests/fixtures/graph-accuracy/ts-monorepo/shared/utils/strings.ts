export function slugify(title: string): string {
  return title.trim().toLowerCase().replace(/\s+/g, "-");
}

export function truncate(text: string, size: number): string {
  return text.length > size ? text.slice(0, size) : text;
}
