export async function runWithConcurrency<T>(
  items: T[],
  worker: (item: T) => Promise<void>,
  concurrency: number
): Promise<void> {
  if (items.length === 0) return;

  const limit = Math.min(Math.max(concurrency, 1), items.length);
  let index = 0;
  const failures: unknown[] = [];
  const runners = Array.from({ length: limit }, async () => {
    while (true) {
      const current = index++;
      if (current >= items.length) return;
      try {
        await worker(items[current]);
      } catch (error) {
        failures.push(error);
      }
    }
  });

  await Promise.all(runners);
  if (failures.length > 0) throw failures[0];
}
