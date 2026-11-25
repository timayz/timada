# The MCP server

An assistant that knows the method can do much of a launch: name the
categories, describe the products, read the reports and say what to change.
The dashboard's MCP server is what it reads and writes through.

Judgment stays with a person. Every writing tool ends in a
[proposal](proposals.md) that waits for approval. No tool approves,
publishes, refunds, or touches payments or secrets.

The protocol and the tools are `dashboard/src/mcp.rs` and
`dashboard/src/mcp/tools.rs`; the keys are in `crates/tenancy`
(`command/agent_key.rs`).

## Turning it on (operator)

```toml
[mcp]
enabled = true
```

Off by default: `POST /mcp` then answers 404 and no page offers a key.

## A key (user)

On **Account → Agent keys** a user makes a key: a label, and for each shop
they are a member of, the permissions the key gets there, among the ones
they hold. The token (`mk_…`) is shown once.

- **Never wider than its user.** A key is made only with permissions the
  user holds in the shop. On every call the user's own permission is
  checked again: a member who lost a permission, or left the shop, takes
  the key's reach with them.
- **Withdrawn at any time**, from the same page. The next call is turned
  away.
- **Twenty keys a user**, twenty shops a key.
- **Last use and log.** The page shows the last day a key was used, how
  many calls it made that day, and its last twenty, refused ones marked.

## Connecting a client

The server speaks MCP over HTTP at `POST {dashboard}/mcp`, one JSON-RPC
request a call, answered in the response body. It keeps no session and
streams nothing, so any node answers any call.

```json
{
  "mcpServers": {
    "timada": {
      "type": "http",
      "url": "https://timada.example/mcp",
      "headers": { "Authorization": "Bearer mk_…" }
    }
  }
}
```

| Request | Answer |
|---|---|
| `initialize` | The protocol revision (the client's when it is `2025-06-18`, `2025-03-26` or `2024-11-05`, else `2025-06-18`), `tools` as the one capability |
| `ping` | `{}` |
| `tools/list` | The tools below, each with its JSON schema |
| `tools/call` | The tool's answer as one text block of JSON; a refusal as text with `isError: true` |
| a notification | 202, no answer |

| Status | When |
|---|---|
| 401 | no key, or one that does not stand |
| 429 | more than 30 calls of a tool in a minute with one key, every node together |
| 400 | not JSON, or not a request (batches are not taken) |

## The tools

A tool about a shop takes the shop's slug as `shop`; `list_shops` names the
slugs. It needs its permissions both of the key and of the key's user.

### Reading

| Tool | Answers | Needs |
|---|---|---|
| `list_shops` | The shops the key covers, with what it may do in each | none |
| `list_categories` | A shop's categories, their tree and product counts | `products:read` |
| `list_products` | A shop's products, 100 at most, by search, category or status | `products:read` |
| `get_product` | One product: texts, prices, kept out of ads, margin and break-even | `products:read` |
| `sales_report` | Orders, revenue, refunds, profit and Google Ads cost over the last days | `orders:read` |
| `ads_report` | Google Ads: totals, campaigns, ad groups, products, searches | `orders:read` |
| `search_console` | Clicks, impressions, position, top queries and pages | `pages:read` |
| `merchant_center` | How many products Google accepts, looks at, refuses | `pages:read` |
| `launch_path` | Every step of [the launch path](launch.md), with what was found | `orders:read` |
| `checkpoints` | The target, the checkpoints read, the next one | `orders:read` |
| `list_studies` | The niche studies of the key's user | none |
| `list_proposals` | What waits for a person on a shop | `proposals:prepare` |

Amounts are text with two decimals (`19.90`), beside their currency.

### Preparing

Each answers with the id of the proposal it prepared, which waits under
**Proposals** on the dashboard. Nothing changes in the shop until a person
with the right to approve does.

| Tool | Proposes | Needs |
|---|---|---|
| `propose_product_change` | A product's name, description, search engines' title and description, price | `proposals:prepare`, `products:read` |
| `propose_category_change` | A category's name, description, text below the products, search engines' title and description | `proposals:prepare`, `products:read` |
| `propose_category` | A new category, created as a draft once approved | `proposals:prepare`, `products:read` |
| `propose_ads_exclusion` | A product kept out of ads; it stays in the shop | `proposals:prepare`, `orders:read` |

A proposal an agent prepared says so: it is prepared under the key's user,
and its source reads `agent {the key's label}`, which the waiting list is
grouped by. What the shop would refuse (a price that is no amount, a name
too long) is told to the agent in the shop's words, and nothing is
prepared.

## What is not there yet

The issue (#695) lists more than this first set; the server is made to
grow a tool at a time (`TOOLS` and `call` in `tools.rs`).

- The tools that spend money: a keyword fetch, a supplier search (#693), a
  text draft (#694). They need the caps of the user and of the platform
  held around an agent's calls.
- Meta fields and prices in bulk, an import from a supplier search, a
  study's hand checks.
- A call's arguments in the log: it keeps the tool, the shop and whether it
  was answered.
