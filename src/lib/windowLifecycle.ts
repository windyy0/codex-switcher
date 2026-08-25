export const CLOSE_MAIN_WINDOW_COMMAND = "close_main_window";

export type BackendInvoker = (
  command: string,
  args?: Record<string, unknown>
) => Promise<unknown>;

export async function requestMainWindowClose(
  invokeBackend: BackendInvoker
): Promise<void> {
  await invokeBackend(CLOSE_MAIN_WINDOW_COMMAND);
}
