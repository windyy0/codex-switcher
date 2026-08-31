export interface SingleFlight<T> {
  isRunning(): boolean;
  run(operation: () => Promise<T>): Promise<T>;
}

export function createSingleFlight<T>(): SingleFlight<T> {
  let inFlight: Promise<T> | null = null;

  return {
    isRunning() {
      return inFlight !== null;
    },
    run(operation) {
      if (inFlight) return inFlight;

      const operationPromise = Promise.resolve().then(operation);
      const request = operationPromise.finally(() => {
        if (inFlight === request) inFlight = null;
      });
      inFlight = request;
      return request;
    },
  };
}

export async function withTimeout<T>(
  operation: Promise<T>,
  timeoutMs: number,
  message: string
): Promise<T> {
  let timeoutId: ReturnType<typeof globalThis.setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timeoutId = globalThis.setTimeout(() => reject(new Error(message)), timeoutMs);
  });

  try {
    return await Promise.race([operation, timeout]);
  } finally {
    if (timeoutId !== undefined) globalThis.clearTimeout(timeoutId);
  }
}
