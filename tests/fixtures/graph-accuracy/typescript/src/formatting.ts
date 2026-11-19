export function render(text: string): string {
  return text.toUpperCase();
}

export function pad(text: string, width: number): string {
  return render(text).padEnd(width);
}
