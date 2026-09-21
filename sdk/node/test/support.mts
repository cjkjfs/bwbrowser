import { BwbrowserClient } from "../src/index.mts";
import { FakeBwbrowser } from "./fake-bwbrowser.mts";

export const TOKEN = "test-token-abc123";

/** Start a fake app, point a client at it, and always shut the server down. */
export async function withClient<T>(
  work: (client: BwbrowserClient, fake: FakeBwbrowser) => Promise<T>,
): Promise<T> {
  const fake = await new FakeBwbrowser().start();
  try {
    const client = new BwbrowserClient({
      token: TOKEN,
      port: fake.port,
      timeoutMs: 5_000,
      env: {},
    });
    return await work(client, fake);
  } finally {
    await fake.stop();
  }
}
