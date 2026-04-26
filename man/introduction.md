# grex

> **A nested meta-repo manager.** Track many git repos as a single graph,
> sync them in parallel, and drive every operation from a shell, CI, or an
> LLM agent speaking MCP.

`grex` is what you reach for when one git repo is no longer enough — when
you have a tree of related repos (a workspace, a fleet of services, a set
of dotfiles + plugins + tools) and you want **one declarative source of
truth** that says which repos belong, where they live on disk, and how
they're kept in sync.

It is **not** a dev-environment installer, not a package manager, not
`mise` / `asdf`. It manages *repos*, not language toolchains.

## In 30 seconds

```sh
cargo install grex-cli                       # binary is `grex`
grex init                                    # creates grex.jsonl in cwd
grex add https://github.com/you/svc-a        # registers + clones a sub-repo
grex add https://github.com/you/svc-b
grex sync                                    # parallel pull/clone for all
grex status --json                           # machine-readable state
```

`grex.jsonl` (intent) and `grex.lock.jsonl` (resolved state) are the only
files you commit to your meta-repo. Everything else `grex` does — clone,
pull, run actions, talk MCP — is reproducible from those two files.

## What you get

- **One CLI, twelve frozen verbs.** `init add rm ls status sync update
  doctor serve import run exec`. Universal `--json --plain --dry-run
  --parallel <N> --filter <EXPR>` on every verb.
  See the [CLI reference](./reference/cli.md).
- **Pack contract.** Any git repo with a `.grex/pack.yaml` is a *pack*.
  Three built-in pack-types ship; the [plugin API](./reference/plugin-api.md)
  lets you add more without forking. Read the [pack spec](./concepts/pack-spec.md).
- **Reproducible manifest.** Newline-delimited JSON, schema-versioned per
  row. See [manifest](./concepts/manifest.md).
- **MCP server built-in.** `grex serve` speaks native MCP 2025-06-18 over
  stdio — every non-`serve` verb becomes a tool call, no custom dialect.
  See [MCP reference](./reference/mcp.md).
- **Parallel scheduler with a Lean4 invariant proof.** Bounded semaphore
  + per-pack `.grex-lock` + `fd-lock` manifest guard; "no double-lock" is
  mechanised. See [concurrency](./concepts/concurrency.md).
- **Migration from `REPOS.json` meta-repos** via
  `grex import --from-repos-json`. See [migration](./guides/migration.md).

## Read next

- New here? Start with [Goals](./concepts/goals.md) then
  [Architecture](./concepts/architecture.md).
- Writing a pack? Read the [Pack spec](./concepts/pack-spec.md) and
  [Pack template](./reference/pack-template.md).
- Driving grex from an agent? Jump to [MCP](./reference/mcp.md) and the
  [CLI JSON output](./reference/cli-json.md) reference.
- Curious how it's built? See the [engineering
  handbook](./guides/engineering.md) and the [roadmap](./internals/roadmap.md).

API reference (rustdoc): [`grex-core`](https://docs.rs/grex-core) ·
[`grex-mcp`](https://docs.rs/grex-mcp).

> **Heads up:** the published crate is `grex-cli`; the installed binary
> is `grex`. If pemistahl's unrelated `grex` (regex-from-test-cases) is
> already on your `PATH`, pass `--force` to `cargo install grex-cli` or
> rename the other binary first.
