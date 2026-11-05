/** Resolves after the next paint, so state set just before has been committed and laid out. */
export function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}
