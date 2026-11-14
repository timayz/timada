// Which server a worker drives. `make e2e` (scripts/e2e.sh) starts E2E_SHARDS
// servers, each on its own database and ports, and Playwright runs as many
// workers (playwright.config.ts): worker N drives server N alone, so two spec
// files never share the first registered user or the ports of the fake
// Stripe, VIES and AliExpress. Playwright numbers the workers that run at the
// same time from 0 (TEST_PARALLEL_INDEX), and sets it before a worker loads
// the config and the specs.

/// How many servers the run has.
export const SHARDS = Number(process.env.E2E_SHARDS ?? 1);

/// This worker's server: 0 with one server, and in the runner process.
export const SHARD = SHARDS > 1 ? Number(process.env.TEST_PARALLEL_INDEX ?? 0) : 0;

if (SHARD >= SHARDS) {
  throw new Error(`worker ${SHARD} has no server: run at most E2E_SHARDS (${SHARDS}) workers`);
}

/// Where server `shard` answers, through the proxy.
export function shardUrl(shard: number): string {
  return SHARDS > 1
    ? `https://s${shard}.timada.localhost`
    : (process.env.E2E_BASE_URL ?? "https://timada.localhost");
}

/// The port of a stand-in this worker's server calls: `base` for server 0,
/// 100 further on for each next one, as scripts/e2e.sh configures them.
export function standInPort(base: number): number {
  return base + 100 * SHARD;
}

/// This worker's server's store API.
export const STORE_API_PORT = 4321 + SHARD;
