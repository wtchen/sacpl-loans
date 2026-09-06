/**
 * Hard timeout around backend calls. Whatever happens on the Rust side
 * (slow catalog, bridge reload, suspended webview), the UI recovers and
 * the buttons can never stay stuck.
 */
export function withTimeout<T>(p: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`${label} is taking too long. The catalog may be busy — please try again.`)),
      ms
    );
    p.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        clearTimeout(timer);
        reject(e);
      }
    );
  });
}
